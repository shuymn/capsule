use capsule_core::{config::ModuleDef, module::preset_module_defs};

const SLOT_HELP: &str =
    "# slot: \"line1\" (default, after git) | \"line2\" (input line, before time)\n\n";

#[derive(serde::Serialize)]
#[cfg_attr(test, derive(serde::Deserialize))]
struct PresetOutput {
    module: Vec<ModuleDef>,
}

pub fn run() -> anyhow::Result<()> {
    print!("{}", render_preset_output()?);
    Ok(())
}

fn render_preset_output() -> anyhow::Result<String> {
    let output = PresetOutput {
        module: preset_module_defs(),
    };
    let toml = toml::to_string(&output)?;
    Ok(format!("{SLOT_HELP}{toml}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_output_includes_slot_help() -> anyhow::Result<()> {
        let rendered = render_preset_output()?;
        assert!(rendered.starts_with(SLOT_HELP));
        assert!(rendered.contains("slot = \"line1\""));
        Ok(())
    }

    #[test]
    fn test_preset_output_round_trips() -> Result<(), Box<dyn std::error::Error>> {
        let presets = preset_module_defs();
        let output = PresetOutput {
            module: presets.clone(),
        };
        let serialized = toml::to_string(&output)?;
        assert!(
            serialized.contains("[[module]]"),
            "output should contain [[module]] array-of-tables"
        );

        let deserialized: PresetOutput = toml::from_str(&serialized)?;
        assert_eq!(deserialized.module, presets);
        Ok(())
    }
}
