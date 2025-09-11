#!/bin/sh

cargo build --package ddo_launcher_dummy --release --target i686-pc-windows-gnu 
cargo build --package ddo_launcher_native --release 