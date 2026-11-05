# Dock API 연결

winit/eframe 등 host가 application delegate를 설치한 뒤 주 스레드에서 연결합니다. 기존 delegate 객체와 클래스는 변경하지 않습니다.

```rust,ignore
use fmv_macos_events::{DockEvent, DockInbox, DockMenuRegistration};

let inbox = DockInbox::default();
let repaint = ctx.clone();
inbox.set_wake(move || repaint.request_repaint());
let dock = DockMenuRegistration::install(localized_new_window, inbox.clone())?;

// UI 루프에서 기존 새 창 명령으로 전달합니다.
for event in inbox.drain() {
    match event {
        DockEvent::NewWindow => dispatch_existing_new_window_command(),
    }
}

// 인터페이스 언어가 바뀌었을 때만 갱신합니다.
dock.set_title(new_localized_title)?;
// 종료 시 주 스레드에서 해제합니다.
drop(dock);
```

`DockInbox`와 guard는 `!Send + !Sync`입니다. `install(&str, DockInbox)`와 `set_title(&self, &str)`는 `Result`를 반환합니다. 제목은 비어 있거나 제어문자를 포함하거나 UTF-8 1,024바이트를 넘으면 거절합니다. 정상 Unicode 문구는 변경하지 않습니다.

wake는 UI 작업을 **예약**하는 짧은 콜백이어야 합니다. 콜백에서 문서나 창을 직접 만들지 않습니다. 동기 재진입의 wake 알림만 합치며 모든 이벤트는 각각 보존합니다. 앱은 실제 클릭을 임의로 debounce하지 않고 기존 새 창 명령의 정책을 적용합니다.

## 실패와 해제

- 다른 `applicationDockMenu:` 구현이 있으면 `ExistingDockMenu`로 실패합니다. inherited selector도 포함합니다.
- 한 프로세스/주 스레드에는 활성 등록 하나만 허용합니다. 설치 중 재진입도 거절합니다.
- guard 해제 후 영구 trampoline은 nil을 반환합니다. 이미 반환한 메뉴의 target도 nil로 바꿉니다.
- Dock이 복사해 둔 target/action의 늦은 전달까지 유효한 수명을 보장하기 위해, **token만 가진 작은 action target 하나를 등록마다 프로세스 수명 동안 보존**합니다. 메뉴·inbox·delegate는 해제합니다. 늦은 action은 token 불일치로 아무 작업도 하지 않습니다.
- 재설치는 같은 trampoline만 재사용하며 새 token을 발급합니다. 이전 guard는 새 등록을 제거할 수 없습니다.
- 기존 `Event::{Open, Quit}`와 Apple-event 공개 API는 유지됩니다. 기존 Inbox도 wake 호출 전에 내부 borrow를 해제합니다.

## 실제 앱에서 확인할 범위

단위 검사는 NSApplication이나 창을 만들지 않습니다. 실제 Dock 표시와 클릭, AppKit의 optional selector 조회 캐시, 언어 변경 후 표시, 메뉴를 연 상태에서 guard를 해제한 결과는 연결한 앱에서 확인해야 합니다. delegate 메서드를 직접 호출하는 검사로 실제 Dock 검사를 대체하지 않습니다.

현재 구현은 delegate를 다시 설정하지도 않습니다. AppKit이 이미 설치된 delegate의 optional selector를 재인식하지 않는 환경이면, delegate를 잠시 nil로 바꾸거나 객체·클래스를 교체하는 우회는 사용하지 않습니다. 그런 실패가 확인되면 host의 기존 delegate 클래스에 typed callback을 정적으로 추가하는 fork를 검토해야 합니다. 실제 검증 전까지 이 경계가 해결됐다고 주장하지 않습니다.
