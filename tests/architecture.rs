//! Keep the checked-in layer contract in the regular repository test path.
#[test]
fn backend_boundaries() {
    let status = std::process::Command::new("python3")
        .arg("tests/architecture.py")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .status()
        .expect("run architecture checks");
    assert!(status.success());
}
