//! Check the public preset output.

use std::process::Command;

#[test]
fn preset_prints_the_example_configuration() -> Result<(), Box<dyn std::error::Error>> {
    let preset = Command::new(env!("CARGO_BIN_EXE_capsule"))
        .arg("preset")
        .output()?;
    assert!(preset.status.success());
    assert_eq!(
        preset.stdout,
        include_bytes!("../../../examples/config.toml")
    );
    Ok(())
}
