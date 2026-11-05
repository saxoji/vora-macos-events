//! A Dock-menu extension that preserves an existing NSApplication delegate.

use crate::{
    queue::{Queue, unwind_boundary},
    registry::Registry,
};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Imp, ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{NSApplication, NSApplicationDelegate, NSMenu, NSMenuItem};
use objc2_foundation::{MainThreadMarker, NSObject, NSObjectProtocol, NSString, ns_string};
use std::{cell::RefCell, fmt, marker::PhantomData, ptr, rc::Rc};

/// One event per actual Dock command. It deliberately does not extend `Event`,
/// preserving existing exhaustive Apple-event matches in applications.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DockEvent {
    NewWindow,
}

/// Main-thread messages. Clones share the queue and this type is !Send + !Sync.
#[derive(Clone, Default)]
pub struct DockInbox(Queue<DockEvent>);

impl DockInbox {
    pub fn drain(&self) -> Vec<DockEvent> {
        self.0.drain()
    }

    /// Schedule UI work (e.g. request_repaint); do not open a window in the native
    /// callback. Recursive wake notifications coalesce, while all events remain.
    pub fn set_wake(&self, wake: impl Fn() + 'static) {
        self.0.set_wake(wake);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DockMenuError {
    NotMainThread,
    NoApplicationDelegate,
    AlreadyRegistered,
    ExistingDockMenu,
    MethodRegistrationFailed,
    InvalidTitle,
    RegistrationInactive,
    DelegateChanged,
}

impl fmt::Display for DockMenuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotMainThread => "Dock menus require the main thread",
            Self::NoApplicationDelegate => "Install the event loop's application delegate first",
            Self::AlreadyRegistered => "A Dock menu registration is active or being installed",
            Self::ExistingDockMenu => "The application delegate already implements a Dock menu",
            Self::MethodRegistrationFailed => "Could not register the Dock menu callback",
            Self::InvalidTitle => "The Dock menu title must be a non-empty single line",
            Self::RegistrationInactive => "The Dock menu registration is no longer active",
            Self::DelegateChanged => "The application delegate changed after registration",
        })
    }
}

impl std::error::Error for DockMenuError {}

struct DockState {
    delegate: Retained<ProtocolObject<dyn NSApplicationDelegate>>,
    menu: Retained<NSMenu>,
    item: Retained<NSMenuItem>,
    target: Retained<DockActionTarget>,
    inbox: DockInbox,
}

impl Drop for DockState {
    fn drop(&mut self) {
        // SAFETY: Also cover incomplete setup and main-thread TLS teardown.
        // NSMenuItem does not retain its target; clear it before the target is
        // released even if an autoreleased/retained menu outlives this state.
        unsafe { self.item.setTarget(None) };
        // AppKit/Dock may already have copied the target/action pair while a
        // menu was tracking. Keep only this tiny token-bearing target alive for
        // the process lifetime, so a late delivery reaches a valid object and
        // fails the registry token check. Do not retain menu, delegate or inbox.
        // An untracked raw +1 is intentional retirement, not a borrowed pointer.
        let _retired_target = Retained::into_raw(self.target.clone());
    }
}

thread_local! {
    static REGISTRY: RefCell<Registry<DockState>> = RefCell::new(Registry::default());
}

define_class!(
    // SAFETY: NSObject has no subclass invariants. The target and its token are
    // created, retained, called and dropped on the main thread only.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = u64]
    #[name = "FmvDockMenuActionTargetV1"]
    struct DockActionTarget;

    unsafe impl NSObjectProtocol for DockActionTarget {}

    impl DockActionTarget {
        // SAFETY: AppKit actions take one nullable Objective-C sender and return
        // void. The whole body catches Rust unwinding before returning to AppKit.
        #[unsafe(method(fmvNewWindow:))]
        fn new_window(&self, _sender: Option<&AnyObject>) {
            let _ = unwind_boundary(|| {
                let Some(mtm) = MainThreadMarker::new() else { return };
                let active = REGISTRY.with(|registry| registry.borrow().active());
                let Some((token, state)) = active else { return };
                if token != *self.ivars() || !ptr::eq(&*state.target, self) {
                    return;
                }
                let Some(current) = NSApplication::sharedApplication(mtm).delegate() else { return };
                if Retained::as_ptr(&current) != Retained::as_ptr(&state.delegate) {
                    return;
                }
                // Retain all native objects before invoking user wake code. A
                // wake may drop the registration or install a replacement.
                state.inbox.0.push(DockEvent::NewWindow);
            });
        }
    }
);

type DockMenuMethod =
    unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut NSApplication) -> *mut NSMenu;

unsafe extern "C-unwind" fn application_dock_menu(
    receiver: *mut AnyObject,
    _cmd: Sel,
    _sender: *mut NSApplication,
) -> *mut NSMenu {
    let menu = unwind_boundary(|| {
        MainThreadMarker::new()?;
        let active = REGISTRY.with(|registry| registry.borrow().active());
        let (_, state) = active?;
        if Retained::as_ptr(&state.delegate).cast::<AnyObject>() != receiver.cast_const() {
            return None;
        }
        Some(state.menu.clone())
    })
    .flatten();
    // applicationDockMenu: is outside Objective-C's retained-return method
    // families. Give AppKit +0, while allowing a concurrent guard drop in a
    // nested main-thread event loop to release our owning reference safely.
    menu.map_or(ptr::null_mut(), Retained::autorelease_return)
}

fn dock_imp() -> Imp {
    // SAFETY: class_addMethod erases method signatures to Imp. The selector's
    // @@:@ encoding below and DockMenuMethod agree: object, self, SEL, object.
    unsafe { std::mem::transmute::<DockMenuMethod, Imp>(application_dock_menu as DockMenuMethod) }
}

fn install_selector(class: &AnyClass) -> Result<(), DockMenuError> {
    let selector = sel!(applicationDockMenu:);
    if let Some(method) = class.instance_method(selector) {
        return if std::ptr::fn_addr_eq(method.implementation(), dock_imp()) {
            Ok(()) // The permanent, currently inactive trampoline may be reused.
        } else {
            Err(DockMenuError::ExistingDockMenu)
        };
    }
    // SAFETY: We add only an absent public instance selector with its exact ABI
    // to an already registered class. Never replace an IMP, delegate or class.
    let added = unsafe {
        objc2::ffi::class_addMethod(
            ptr::from_ref(class).cast_mut(),
            selector,
            dock_imp(),
            c"@@:@".as_ptr(),
        )
    };
    if added.as_bool() {
        Ok(())
    } else {
        Err(DockMenuError::MethodRegistrationFailed)
    }
}

fn validate_title(title: &str) -> Result<(), DockMenuError> {
    if title.trim().is_empty() || title.len() > 1024 || title.chars().any(char::is_control) {
        Err(DockMenuError::InvalidTitle)
    } else {
        Ok(())
    }
}

/// Keep this !Send/!Sync guard until the event loop stops. Dropping it removes
/// its action target and makes the permanent native trampoline return nil.
/// Install after the host (such as winit) has installed its delegate.
pub struct DockMenuRegistration {
    token: u64,
    _main_thread: PhantomData<Rc<()>>,
}

impl DockMenuRegistration {
    pub fn install(title: &str, inbox: DockInbox) -> Result<Self, DockMenuError> {
        let mtm = MainThreadMarker::new().ok_or(DockMenuError::NotMainThread)?;
        validate_title(title)?;
        let token = REGISTRY
            .with(|registry| registry.borrow_mut().reserve())
            .ok_or(DockMenuError::AlreadyRegistered)?;
        // This guard also clears an incomplete reservation on every error path.
        let guard = Self {
            token,
            _main_thread: PhantomData,
        };
        let app = NSApplication::sharedApplication(mtm);
        let delegate = app.delegate().ok_or(DockMenuError::NoApplicationDelegate)?;
        install_selector(AsRef::<AnyObject>::as_ref(&*delegate).class())?;
        let allocated = DockActionTarget::alloc(mtm).set_ivars(token);
        // SAFETY: NSObject init completes the allocated, main-thread subclass.
        let target: Retained<DockActionTarget> = unsafe { msg_send![super(allocated), init] };
        let title = NSString::from_str(title);
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        // SAFETY: fmvNewWindow: has the nullable-sender, void-return AppKit action
        // signature above. No keyboard equivalent or responder-chain fallback.
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                &title,
                Some(sel!(fmvNewWindow:)),
                ns_string!(""),
            )
        };
        // SAFETY: Our retained target implements the action and stays alive until
        // after the item's target is explicitly cleared by the guard's Drop.
        unsafe { item.setTarget(Some(&target)) };
        menu.addItem(&item);
        let state = Rc::new(DockState {
            delegate,
            menu,
            item,
            target,
            inbox,
        });
        if !REGISTRY.with(|registry| registry.borrow_mut().publish(token, state)) {
            return Err(DockMenuError::RegistrationInactive);
        }
        Ok(guard)
    }

    /// Update the existing command label after an interface-language change.
    pub fn set_title(&self, title: &str) -> Result<(), DockMenuError> {
        let mtm = MainThreadMarker::new().ok_or(DockMenuError::NotMainThread)?;
        validate_title(title)?;
        let (_, state) = REGISTRY
            .with(|registry| registry.borrow().active())
            .filter(|(token, _)| *token == self.token)
            .ok_or(DockMenuError::RegistrationInactive)?;
        let current = NSApplication::sharedApplication(mtm)
            .delegate()
            .ok_or(DockMenuError::DelegateChanged)?;
        if Retained::as_ptr(&current) != Retained::as_ptr(&state.delegate) {
            return Err(DockMenuError::DelegateChanged);
        }
        state.item.setTitle(&NSString::from_str(title));
        Ok(())
    }
}

impl Drop for DockMenuRegistration {
    fn drop(&mut self) {
        let _ = unwind_boundary(|| {
            let state = REGISTRY.with(|registry| registry.borrow_mut().remove(self.token));
            if let Some(state) = state {
                // SAFETY: Clearing this non-owning target prevents an already
                // returned/autoreleased menu from sending to a released target.
                // No registry borrow is held while calling AppKit or dropping it.
                unsafe { state.item.setTarget(None) };
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2::ClassType;
    use objc2::runtime::ClassBuilder;

    #[test]
    fn labels_preserve_localized_text_and_reject_empty_or_control_lines() {
        for title in ["새 창", "New Window", "新規ウインドウ", "نافذة جديدة"] {
            assert!(validate_title(title).is_ok());
        }
        for title in ["", "  ", "New\nWindow", "New\0Window"] {
            assert_eq!(validate_title(title), Err(DockMenuError::InvalidTitle));
        }
    }

    #[test]
    fn selector_is_added_once_without_replacing_class_or_foreign_method() {
        let class = ClassBuilder::new(c"FmvDockMissingSelectorTest246", NSObject::class())
            .unwrap()
            .register();
        let address = ptr::from_ref(class);
        assert!(class.instance_method(sel!(applicationDockMenu:)).is_none());
        assert!(install_selector(class).is_ok());
        assert!(install_selector(class).is_ok());
        assert_eq!(ptr::from_ref(class), address);
        unsafe extern "C-unwind" fn foreign(
            _: *mut AnyObject,
            _: Sel,
            _: *mut NSApplication,
        ) -> *mut NSMenu {
            ptr::null_mut()
        }
        let other = ClassBuilder::new(c"FmvDockForeignSelectorTest246", NSObject::class())
            .unwrap()
            .register();
        let foreign_imp =
            unsafe { std::mem::transmute::<DockMenuMethod, Imp>(foreign as DockMenuMethod) };
        assert!(
            unsafe {
                objc2::ffi::class_addMethod(
                    ptr::from_ref(other).cast_mut(),
                    sel!(applicationDockMenu:),
                    foreign_imp,
                    c"@@:@".as_ptr(),
                )
            }
            .as_bool()
        );
        assert_eq!(
            install_selector(other),
            Err(DockMenuError::ExistingDockMenu)
        );
        assert!(std::ptr::fn_addr_eq(
            other
                .instance_method(sel!(applicationDockMenu:))
                .unwrap()
                .implementation(),
            foreign_imp
        ));
    }

    #[test]
    fn inactive_or_wrong_thread_callback_returns_nil_without_appkit() {
        // No NSApplication or GUI is created. A unit-test thread is not the main
        // thread; either way an empty registry must produce a nil +0 result.
        let result = unsafe {
            application_dock_menu(ptr::null_mut(), sel!(applicationDockMenu:), ptr::null_mut())
        };
        assert!(result.is_null());
        assert_eq!(
            DockMenuRegistration::install("New Window", DockInbox::default()).err(),
            Some(DockMenuError::NotMainThread)
        );
    }
}
