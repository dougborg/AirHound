#[test]
fn help_is_noninteractive_and_socket_is_explicit() {
    let binary = env!("CARGO_BIN_EXE_airhound-tui");
    let output = std::process::Command::new(binary)
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("--socket"));
    assert!(text.contains("never starts radios"));
    assert!(!text.contains('\x1b'));
    assert!(!std::process::Command::new(binary)
        .output()
        .unwrap()
        .status
        .success());
    let output = std::process::Command::new(binary)
        .args(["--socket", "/no-such-airhound-socket"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("interactive stdin and stdout"));
    assert!(!output.stdout.contains(&0x1b));
}
