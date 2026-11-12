# macshell

<img src="assets/macshell-avatar.png" alt="macshell avatar" width="160">

An experimental Windows desktop shell inspired by macOS, built primarily in Rust.

macshell brings together a top menu bar, a Dock and application search with system controls and desktop widgets. It is a personal exploration of desktop interfaces, Windows APIs and graphics programming.

> **Preview software:** window management and animations are experimental. This project is independent and is not affiliated with Apple.

## Try the preview

1. Download `macshell-windows-x64.zip` from [Releases](https://github.com/anintas/macshell/releases).
2. Extract the archive and run `macshell.exe`.
3. To exit, run `macshell.exe --quit` from a terminal in the same folder.

If the Windows taskbar remains hidden after an unexpected exit, run `macshell.exe --restore-taskbar`.

## Features

- A top bar with system status and controls
- A Dock and application launcher
- Search, desktop widgets and window-management experiments
- Glass rendering using Windows graphics APIs

Some features are experimental and may vary between Windows configurations.

## Build from source

You need Windows, Rust with the MSVC target, and the Visual Studio C++ build tools.

```powershell
git clone https://github.com/anintas/macshell.git
cd macshell
cargo build --release -p macshell
```

The executable is written to `target/release/macshell.exe`. Close a running copy before rebuilding, as Windows can lock the executable.

## Project structure

- `crates/macshell` — shell, Windows integration and Slint interface
- `crates/glass` — glass rendering and GPU shader
- `crates/capture` — screen capture
- `crates/macshell/windhawk` — optional experimental animation integration, with upstream attribution in its source

Rust handles the application logic. Slint describes the interface; HLSL implements the GPU shader. The optional Windhawk integration is C++.

## Status

This is a work in progress, not a replacement for the Windows shell. Installation and clean-machine testing are still being developed. No project-wide license has yet been specified; individual third-party components retain their own notices.

