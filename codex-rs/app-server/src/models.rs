use codex_app_server_protocol::Model;
use codex_app_server_protocol::ModelServiceTier;
use codex_app_server_protocol::ModelUpgradeInfo;
use codex_app_server_protocol::ReasoningEffortOption;
use codex_protocol::openai_models::ModelPreset;
use codex_protocol::openai_models::ReasoningEffortPreset;

pub fn supported_models(models: Vec<ModelPreset>, include_hidden: bool) -> Vec<Model> {
    let mut models = models
        .into_iter()
        .map(model_from_preset)
        .collect::<Vec<_>>();

    apply_chatgpt_web_picker_presentation(&mut models);
    ensure_chatgpt_web_picker_aliases(&mut models);

    if !include_hidden {
        models.retain(|model| !model.hidden);
    }
    models
}

fn apply_chatgpt_web_picker_presentation(models: &mut [Model]) {
    for model in models {
        match model.id.as_str() {
            // These canonical routes stay available internally for saved threads, native routing,
            // and subagents. The desktop picker gets the three fixed-effort aliases below instead,
            // because each effort owns a different ChatGPT context/compaction budget.
            "chatgpt-web/gpt-5.6-sol-instant" | "chatgpt-web/gpt-5.6-sol" => {
                model.hidden = true;
            }
            "chatgpt-web/light" => {
                model.hidden = false;
                model.display_name = "GPT-5.6 Sol (Web) - Leve".to_string();
                model.description = "ChatGPT Web Sol em modo Leve/Instant.".to_string();
            }
            "chatgpt-web/medium" => {
                model.hidden = false;
                model.display_name = "GPT-5.6 Sol (Web) - M\u{00e9}dio".to_string();
                model.description = "ChatGPT Web Sol em modo M\u{00e9}dio.".to_string();
            }
            "chatgpt-web/high" => {
                model.hidden = false;
                model.display_name = "GPT-5.6 Sol (Web) - Alto".to_string();
                model.description = "ChatGPT Web Sol em modo Alto.".to_string();
            }
            _ => {}
        }
    }
}

fn ensure_chatgpt_web_picker_aliases(models: &mut Vec<Model>) {
    let Some(template) = models
        .iter()
        .find(|model| model.id == "gpt-5.6-sol")
        .cloned()
    else {
        return;
    };

    for (id, display_name, description, effort) in [
        (
            "chatgpt-web/light",
            "GPT-5.6 Sol (Web) - Leve",
            "ChatGPT Web Sol em modo Leve/Instant.",
            codex_protocol::openai_models::ReasoningEffort::Low,
        ),
        (
            "chatgpt-web/medium",
            "GPT-5.6 Sol (Web) - Médio",
            "ChatGPT Web Sol em modo Médio.",
            codex_protocol::openai_models::ReasoningEffort::Medium,
        ),
        (
            "chatgpt-web/high",
            "GPT-5.6 Sol (Web) - Alto",
            "ChatGPT Web Sol em modo Alto.",
            codex_protocol::openai_models::ReasoningEffort::High,
        ),
    ] {
        if models.iter().any(|model| model.id == id) {
            continue;
        }

        let mut alias = template.clone();
        alias.id = id.to_string();
        alias.model = id.to_string();
        alias.display_name = display_name.to_string();
        alias.description = description.to_string();
        alias.hidden = false;
        alias.is_default = false;
        alias.upgrade = None;
        alias.upgrade_info = None;
        alias.availability_nux = None;
        alias.default_reasoning_effort = effort;
        alias
            .supported_reasoning_efforts
            .retain(|option| option.reasoning_effort == effort);
        alias.available_access_programs = None;
        models.push(alias);
    }
}

fn model_from_preset(preset: ModelPreset) -> Model {
    Model {
        id: preset.id.to_string(),
        model: preset.model.to_string(),
        upgrade: preset.upgrade.as_ref().map(|upgrade| upgrade.id.clone()),
        upgrade_info: preset.upgrade.as_ref().map(|upgrade| ModelUpgradeInfo {
            model: upgrade.id.clone(),
            upgrade_copy: upgrade.upgrade_copy.clone(),
            model_link: upgrade.model_link.clone(),
            migration_markdown: upgrade.migration_markdown.clone(),
            retirement_at: upgrade
                .retirement_at
                .as_ref()
                .map(chrono::DateTime::timestamp),
        }),
        availability_nux: preset.availability_nux.map(Into::into),
        display_name: preset.display_name.to_string(),
        description: preset.description.to_string(),
        model_specialty: preset.model_specialty,
        hidden: !preset.show_in_picker,
        supported_reasoning_efforts: reasoning_efforts_from_preset(
            preset.supported_reasoning_efforts,
        ),
        default_reasoning_effort: preset.default_reasoning_effort,
        input_modalities: preset.input_modalities,
        supports_personality: preset.supports_personality,
        multi_agent_version: preset.multi_agent_version.map(Into::into),
        additional_speed_tiers: preset.additional_speed_tiers,
        service_tiers: preset
            .service_tiers
            .into_iter()
            .map(|service_tier| ModelServiceTier {
                id: service_tier.id,
                name: service_tier.name,
                description: service_tier.description,
            })
            .collect(),
        default_service_tier: preset.default_service_tier,
        available_access_programs: preset.available_access_programs.map(Into::into),
        is_default: preset.is_default,
    }
}

fn reasoning_efforts_from_preset(
    efforts: Vec<ReasoningEffortPreset>,
) -> Vec<ReasoningEffortOption> {
    efforts
        .into_iter()
        .map(|preset| ReasoningEffortOption {
            reasoning_effort: preset.effort,
            description: preset.description,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::supported_models;
    use codex_core::test_support::all_model_presets;
    use codex_protocol::openai_models::ModelPreset;

    fn preset(id: &str, show_in_picker: bool) -> ModelPreset {
        let mut preset = all_model_presets()
            .first()
            .expect("bundled model catalog should not be empty")
            .clone();
        preset.id = id.to_string();
        preset.model = id.to_string();
        preset.display_name = id.to_string();
        preset.description = id.to_string();
        preset.show_in_picker = show_in_picker;
        preset
    }

    #[test]
    fn web_picker_shows_fixed_effort_aliases_and_hides_technical_rows() {
        let models = supported_models(
            vec![
                preset("gpt-5.6-sol", true),
                preset("chatgpt-web/gpt-5.6-sol-instant", true),
                preset("chatgpt-web/gpt-5.6-sol", true),
                preset("chatgpt-web/light", false),
                preset("chatgpt-web/medium", false),
                preset("chatgpt-web/high", false),
            ],
            false,
        );

        let ids = models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            vec![
                "gpt-5.6-sol",
                "chatgpt-web/light",
                "chatgpt-web/medium",
                "chatgpt-web/high",
            ]
        );

        let light = models
            .iter()
            .find(|model| model.id == "chatgpt-web/light")
            .expect("light Web alias should be visible");
        let medium = models
            .iter()
            .find(|model| model.id == "chatgpt-web/medium")
            .expect("medium Web alias should be visible");
        let high = models
            .iter()
            .find(|model| model.id == "chatgpt-web/high")
            .expect("high Web alias should be visible");

        assert_eq!(light.display_name, "GPT-5.6 Sol (Web) - Leve");
        assert_eq!(medium.display_name, "GPT-5.6 Sol (Web) - Médio");
        assert_eq!(high.display_name, "GPT-5.6 Sol (Web) - Alto");
    }

    #[test]
    fn web_picker_synthesizes_aliases_when_catalog_omits_web_rows() {
        let models = supported_models(vec![preset("gpt-5.6-sol", true)], false);

        for (id, display_name) in [
            ("chatgpt-web/light", "GPT-5.6 Sol (Web) - Leve"),
            ("chatgpt-web/medium", "GPT-5.6 Sol (Web) - Médio"),
            ("chatgpt-web/high", "GPT-5.6 Sol (Web) - Alto"),
        ] {
            let model = models
                .iter()
                .find(|model| model.id == id)
                .expect("Web alias should be synthesized");
            assert_eq!(model.display_name, display_name);
            assert!(!model.hidden);
            assert_eq!(model.supported_reasoning_efforts.len(), 1);
        }
    }

    #[test]
    fn web_picker_include_hidden_keeps_all_internal_routes_available() {
        let models = supported_models(
            vec![
                preset("chatgpt-web/gpt-5.6-sol-instant", true),
                preset("chatgpt-web/gpt-5.6-sol", true),
                preset("chatgpt-web/light", false),
                preset("chatgpt-web/medium", false),
                preset("chatgpt-web/high", false),
            ],
            true,
        );

        assert_eq!(models.len(), 5);
        assert!(
            models
                .iter()
                .find(|model| model.id == "chatgpt-web/gpt-5.6-sol-instant")
                .expect("technical instant route should remain present")
                .hidden
        );
        assert!(
            models
                .iter()
                .find(|model| model.id == "chatgpt-web/gpt-5.6-sol")
                .expect("technical grouped Sol route should remain present")
                .hidden
        );
        for id in [
            "chatgpt-web/light",
            "chatgpt-web/medium",
            "chatgpt-web/high",
        ] {
            assert!(
                !models
                    .iter()
                    .find(|model| model.id == id)
                    .expect("fixed-effort alias should remain present")
                    .hidden
            );
        }
    }
}
