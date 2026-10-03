use std::io::{self, Write};

const EXAMPLE: &str = include_str!("../../../examples/config.toml");

pub fn run() -> anyhow::Result<()> {
    io::stdout().lock().write_all(EXAMPLE.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printed_example_compiles_as_a_schema_v2_plan() -> anyhow::Result<()> {
        let plan = capsule_core::plan::ConfigPlan::parse(EXAMPLE)?;
        assert!(!plan.modules.is_empty());
        Ok(())
    }
}
