# macOS 이벤트와 Dock 메뉴

`fmv-macos-events 0.1.0`의 Apple-event 어댑터에 기존 application delegate를 유지하는 Dock 메뉴 API를 추가한 확장입니다. Rust 앱은 안전한 메시지 큐만 사용합니다.

- 기존 `Event::{Open, Quit}`, `Inbox`, `Registration` API를 유지합니다.
- `DockInbox`, `DockEvent::NewWindow`, `DockMenuRegistration`을 제공합니다.
- 실제 클릭마다 메시지를 한 개 전달하고, 앱이 UI 루프에서 처리하도록 wake를 호출합니다.
- 언어 변경 시 메뉴 제목을 갱신할 수 있습니다.
- 다른 Dock 메뉴 구현이 이미 있으면 대체하지 않고 오류를 반환합니다.

[연결 방법과 실제 앱 검사 범위](DOCK_API.md), [안전성 검토와 단위 검사](AUDIT.md)를 참고하세요.

원본은 [Quetzalcohuatl/fastmarkdownviewer](https://github.com/Quetzalcohuatl/fastmarkdownviewer)의 `crates/macos-events`입니다. 정확한 출처와 원본 파일 해시는 [UPSTREAM.json](UPSTREAM.json)에 기록했습니다. 원본 MIT 또는 Apache-2.0 라이선스를 유지합니다.
