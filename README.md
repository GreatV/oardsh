<div align="center">

<img src="src-tauri/icons/128x128@2x.png" width="112" alt="oardsh" />

# oardsh

Bring [DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) to your desktop.

[简体中文](README.zh-CN.md)

</div>

![oardsh running a dsh session](docs/screenshot.png)

oardsh keeps the familiar dsh Web experience and adds what only a native app can. Everything it contributes lives inside dsh's own Settings page, follows its theme, layout and language, and wears the app's whale mark so oardsh's surfaces are always distinguishable from stock dsh.

- **Tray residency** — closing the window hides it to the tray; the session keeps running. The tray menu shows the window, restarts the server, or quits. A second launch focuses the existing instance.
- **Context meter on hover** — the composer's context ring opens its panel on hover rather than a click, and the panel gains each bucket's share of the used context plus the free remainder. dsh's session stats move in there by default, replacing the truncated line under the composer; **Settings → General** puts them back.
- **Usage statistics** — a dashboard built from local session transcripts: tokens, sessions, messages, active days and current streak over the last 7 or 30 days, a year-long activity heatmap, a daily token trend stacked by model, and each model's share.
- **Network proxy** — **Settings → General** can route network traffic through a proxy. Choose Off, System, or a manual address. Applying the change restarts the server.

## Download

Grab the installer for your platform from [GitHub Releases](https://github.com/GreatV/oardsh/releases) — macOS DMG (Apple Silicon or Intel), Windows x64 installer, or Linux x64 AppImage / Debian / RPM.

## Development

```sh
npm install
npm run tauri dev     # run
npm run tauri build   # build installers for the current platform
```
