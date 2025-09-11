use std::process::Command;

fn main() {
    let output = Command::new("umu-run").output().unwrap();
    println!("{}", std::str::from_utf8(&output.stderr).unwrap());
}
