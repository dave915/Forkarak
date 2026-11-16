# 번들 서체

아래 파일은 공식 저장소에서 변환·서브셋·수정 없이 받은 TrueType 파일이다. 다운로드 기준일은 2026-11-16이다. 각 원본 라이선스의 저작권 고지를 그대로 보존했다.

| 용도 | 파일 | 굵기/스타일 |
| --- | --- | --- |
| UI | `IBMPlexSans-Regular.ttf` | 400 |
| UI | `IBMPlexSans-Medium.ttf` | 500 |
| UI | `IBMPlexSans-SemiBold.ttf` | 600 |
| 코드 | `JetBrainsMono-Regular.ttf` | 400 |
| 코드 | `JetBrainsMono-Medium.ttf` | 500 |
| 코드 | `JetBrainsMono-Italic.ttf` | 400 italic |

UI 서체는 IBM Plex Sans, 코드 서체는 JetBrains Mono다. IBM Plex의 레거시 family name에는 `IBM Plex Sans Medm` 및 `IBM Plex Sans SmBld`도 있으므로 런타임의 굵기 선택을 확인한다. 이 번들은 Plex Sans Korean 패키지를 포함하지 않는다. 한글은 DESIGN.md에 따라 macOS 한글 산세리프로 fallback한다.

## 출처와 라이선스

- IBM Plex: [공식 저장소](https://github.com/IBM/plex), 고정 커밋 [`bc6a7e4b37634eb79785d59781893db63ee3edcb`](https://github.com/IBM/plex/tree/bc6a7e4b37634eb79785d59781893db63ee3edcb). 원본 경로는 `packages/plex-sans/fonts/complete/ttf/<파일명>`이다. [`packages/plex-sans/LICENSE.txt`](https://github.com/IBM/plex/blob/bc6a7e4b37634eb79785d59781893db63ee3edcb/packages/plex-sans/LICENSE.txt)를 [IBM-Plex-OFL.txt](IBM-Plex-OFL.txt)에 보존했다.
- JetBrains Mono: [공식 저장소](https://github.com/JetBrains/JetBrainsMono), 고정 커밋 [`19371302b95d218af43299bce79ddbddd0bc364d`](https://github.com/JetBrains/JetBrainsMono/tree/19371302b95d218af43299bce79ddbddd0bc364d). 원본 경로는 `fonts/ttf/<파일명>`이다. 루트의 [`OFL.txt`](https://github.com/JetBrains/JetBrainsMono/blob/19371302b95d218af43299bce79ddbddd0bc364d/OFL.txt)와 `AUTHORS.txt`를 [JetBrainsMono-OFL.txt](JetBrainsMono-OFL.txt), [JetBrainsMono-AUTHORS.txt](JetBrainsMono-AUTHORS.txt)에 보존했다.

두 서체에는 SIL Open Font License 1.1이 적용된다. 프로젝트 코드의 MIT 라이선스와 구별한다. 재배포 시 동봉한 원문 고지를 함께 유지한다.

원본 다운로드 주소 형식:

```text
https://raw.githubusercontent.com/IBM/plex/bc6a7e4b37634eb79785d59781893db63ee3edcb/packages/plex-sans/fonts/complete/ttf/<파일명>
https://raw.githubusercontent.com/JetBrains/JetBrainsMono/19371302b95d218af43299bce79ddbddd0bc364d/fonts/ttf/<파일명>
```

다운로드한 6개 TTF와 3개 고지 파일의 SHA-256은 `SHA256SUMS`에 기록했다.

```sh
(cd assets/fonts && shasum -a 256 -c SHA256SUMS)
```

## 앱 통합

`crates/app/src/main.rs`에서 정적 포함 시 상대 경로는 `../../../assets/fonts/<파일명>`이다. GPUI의 폰트 등록 API로 바이트를 등록한 뒤 family와 weight를 선택한다. 파일이 저장소나 `.app` 안에 존재하는 것만으로 GPUI 등록이 완료되지는 않는다. 시스템 전체에 폰트를 설치할 필요는 없다.
