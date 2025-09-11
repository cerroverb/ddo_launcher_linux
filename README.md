# DDO Launcher for Linux

A simple replacement for the Windows-only DDON launcher, running natively on Linux
with [umu-launcher](https://github.com/Open-Wine-Components/umu-launcher).

It also includes a "dummy" launcher executable which is a simple no-op executable. 
This can be used to replace the original `ddo_launcher.exe` to prevent the game
from launching that one on exit.

## Usage

Simply run `ddo_launcher_native` from anywhere. Inside the window, you can enter 
your account name and password, and configure the server you wish to play on.

You must also set `DDO.exe Path` and `Wine Prefix Path`. The wine prefix path can be set to 
any empty folder, `umu-launcher` will take care of initializing the prefix on first launch.

## Build

```bash
cargo build --package ddo_launcher_native --release
cargo build --package ddo_launcher_dummy --release --target i686-pc-windows-gnu

./target/release/ddo_launcher_native
```

### Dependencies

`rustup` must be installed and a toolchain set. It can be found at [rustup.rs](https://rustup.rs/)
or in your distribution's package manager.

In order to build the "dummy" launcher executable, a Windows target must also be installed.
On Arch-based distributions, use the following `pacman` command. On other distributions,
find and install the corresponding package.

```bash
sudo pacman -S mingw-w64-gcc
rustup target add i686-pc-windows-gnu
```
