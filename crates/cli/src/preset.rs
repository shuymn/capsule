use capsule_core::{config::ModuleDef, module::preset_module_defs};

#[derive(serde::Serialize)]
#[cfg_attr(test, derive(serde::Deserialize))]
struct PresetOutput {
    module: Vec<ModuleDef>,
}

pub fn run() -> anyhow::Result<()> {
    let output = PresetOutput {
        module: preset_module_defs(),
    };
    let toml = toml::to_string(&output)?;
    print!("{toml}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
