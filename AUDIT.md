# 안전성 검토와 단위 검사

## 출처

원본은 `Quetzalcohuatl/fastmarkdownviewer`의 `crates/macos-events`, commit `d60be0bd516439f1774fce99240cdadce1e4ac3b`에 연결된 crates.io `fmv-macos-events 0.1.0`입니다. 원본 `LICENSE-MIT`와 `LICENSE-APACHE`를 변경하지 않았습니다. 전체 git revision과 Cargo.lock으로 소비자가 의존성을 고정할 수 있습니다.

## 불변 조건

1. **delegate 보존**: 현재 delegate를 retain하고 실제 class를 조회합니다. 없는 공개 selector 하나만 `class_addMethod`로 추가합니다. `setDelegate`, `object_setClass`, `class_replaceMethod`, swizzling은 사용하지 않습니다. inherited selector가 다른 구현이어도 거절합니다.
2. **ABI**: IMP는 `unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut NSApplication) -> *mut NSMenu`, encoding은 `@@:@`입니다. action은 nullable sender 하나와 void 반환입니다.
3. **주 스레드**: 설치·제목 변경과 callback은 `MainThreadMarker`를 확인합니다. guard와 inbox는 Rc를 통해 `!Send + !Sync`입니다.
4. **+0 반환**: `Retained<NSMenu>`를 clone한 뒤 `Retained::autorelease_return`으로 반환합니다. `applicationDockMenu:`는 retained-return family가 아니므로 AppKit에 +0을 제공합니다.
5. **action 수명**: callback이 활성 상태를 Rc로 복제한 뒤 wake를 호출합니다. wake가 guard를 내려도 callback 중 객체는 살아 있습니다. guard/state Drop은 item의 비소유 target을 nil로 바꿉니다. Dock이 이미 복사한 target/action의 늦은 전달을 고려하여 token만 가진 target은 process-lifetime +1 보유로 retire합니다. 전체 menu/inbox/delegate는 보존하지 않습니다.
6. **등록 수명**: 설치 중 상태는 노출하지 않습니다. 증가 token은 overflow 시 재사용하지 않습니다. guard는 자신의 token만 제거합니다. 비활성 trampoline과 다른 receiver는 nil을 반환합니다. stale action은 token/target/current delegate를 검사하여 무시합니다.
7. **재진입**: 이벤트 저장 후 borrow를 끝내고 wake를 호출합니다. 교체된 wake capture의 Drop 역시 borrow 밖에서 수행합니다. 재귀 wake 알림만 합치고 이벤트 개수는 보존합니다.
8. **Rust unwind**: native callback의 Rust body와 wake는 catch_unwind 경계 안에 있습니다. panic payload의 파괴자가 다시 panic할 가능성을 막기 위해 해당 payload는 경계에서 의도적으로 누수시킵니다. `panic=abort`, OOM abort, 외부 Objective-C exception 복구는 보장하지 않습니다. 정상 wake는 비패닉 UI 예약으로 제한합니다.

이 검토는 소비자 앱에 unsafe를 요구하지 않는 API를 대상으로 합니다. 기존 Apple-event 파서 자체의 전체 재감사는 아닙니다.

## 단위 검사

원본 최소 Rust 버전은 1.95입니다. 설치된 cargo와 rustc의 1.95.0 경로를 모두 지정하고 다음 소형 crate 검사만 실행했습니다.

```sh
RUSTC="$(rustup which --toolchain 1.95.0 rustc)"   "$(rustup which --toolchain 1.95.0 cargo)" test --offline --locked --lib -j 1
```

최종 결과는 **9 passed, 0 failed**입니다. 검사 범위는 다음과 같습니다.

- wake에서 drain·콜백 교체·enqueue 재진입
- 교체되는 wake capture의 Drop에서 재진입
- 클릭별 이벤트 개수와 drain
- panic 뒤 이벤트 보존 및 wake 재개
- 설치 중 registry 비노출과 중복 등록 거절
- stale token이 새 등록을 제거하지 않음 및 callback 상태 수명
- 여러 언어의 제목과 잘못된 입력
- synthetic NSObject class에서 selector 추가·재사용·foreign 구현 보존
- 비활성/잘못된 스레드 callback의 nil 반환

synthetic runtime class는 테스트 전용이며 실제 NSApplication delegate가 아닙니다. 테스트는 NSApplication/GUI를 만들지 않습니다. 실제 AppKit +0 수명과 Dock 클릭/추적 중 해제 동작, 이미 설치된 delegate의 selector 캐시는 별도의 실제 앱 gate입니다. registry 검사를 이러한 GUI 검사 통과로 표현하지 않습니다.

## 대안 비교

host의 기존 delegate에 typed Dock callback을 정적으로 추가하면 AppKit selector 캐시 불확실성을 제거할 수 있습니다. 대신 더 큰 windowing dependency fork와 해당 host의 공개 API 변경이 필요합니다. 이 확장은 기존 class와 delegate를 유지하는 작은 추가를 선택했으며, 실제 Dock 표시가 실패하면 delegate 교체 우회 대신 typed host 확장을 검토합니다.

## 공개 근거

- [Apple applicationDockMenu](https://developer.apple.com/documentation/appkit/nsapplicationdelegate/applicationdockmenu(_:))
- [Apple class_addMethod](https://developer.apple.com/documentation/objectivec/class_addmethod(_:_:_:_:))
- objc2 0.6.4 `rc/retained.rs:665–696`: custom method의 +0 autorelease_return 예제.
- objc2 0.6.4 `runtime/mod.rs:835–844`: superclass를 포함한 instance_method 조회.
- winit `a517b3846ee8457d84ee4975110bfda6797c44dc`, `platform_impl/macos/app_state.rs:231–239`: host가 등록한 delegate와 다른 타입이면 panic하므로 교체하지 않음.
