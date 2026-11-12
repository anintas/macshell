# macshell

Experimental Windows desktop shell inspired by macOS. It includes a menu bar, Dock, application search, system controls and widgets.

## Build

Windows, a Rust toolchain with the MSVC target, and Visual Studio C++ build tools are required.

```powershell
cargo build --release -p macshell
```

The executable is `target/release/macshell.exe`. Run it to start the shell. To quit, run `macshell.exe --quit`. If the Windows taskbar remains hidden after an unexpected exit, run `macshell.exe --restore-taskbar`.

This project is experimental. Window decoration and the optional Windhawk component are under development.
