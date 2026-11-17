# 포카락 자산

`icons/spork.svg`는 포카락을 위해 그린 원본 마크다. 64×64 좌표계에서 선 굵기 3.5, 둥근 선 끝, 은색 `#e5eaf0`를 사용한다. 네 개의 짧은 포크 끝과 둥근 숟가락 볼, 일자 손잡이로 구성한다. 외부 이미지나 이모지를 사용하지 않는다.

`icons/app-icon.svg`는 같은 마크를 `#20252c` 바탕에 배치한 macOS 앱 아이콘 원본이다. `icons/Forkarak.icns`는 16, 32, 128, 256, 512pt의 1×/2× 이미지로 만들었다. `icons/app-icon-1024.png`는 검토용 렌더링이다. 이 프로젝트에서 작성한 마크와 스크립트에는 루트의 [MIT 라이선스](../LICENSE)가 적용된다.

macOS와 Xcode command-line tools가 설치된 환경에서 다음 명령으로 재생성한다. Cargo 빌드나 외부 렌더러 설치는 필요 없다.

```sh
./scripts/build-icons.sh
```

스크립트는 `render-icon.swift`에서 AppKit으로 SVG를 각 크기에 직접 렌더링한 후 시스템 `iconutil`로 ICNS를 만든다. 임시 iconset은 정리한다.

폰트 원본과 라이선스는 [fonts/README.md](fonts/README.md)에 기록했다. 앱 번들에 파일을 복사하는 것과 GPUI 텍스트 시스템에 서체를 등록하는 것은 별도 작업이다. 런타임은 필요한 TTF 바이트를 명시적으로 등록해야 한다.

파일 목록과 탭은 [VSCode Icons](icons/vscode/NOTICE.md)의 컬러 SVG를 사용한다. 파일명과 복합 확장자에 따라 선택하며 밝은 테마용 아이콘을 지원한다.
