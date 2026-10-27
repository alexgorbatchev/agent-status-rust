use agent_common::types::TokenUsage;
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ModelPricing {
    pub input_cost_per_token: f64,
    pub output_cost_per_token: f64,
    pub cache_read_input_token_cost: f64,
    pub cache_creation_input_token_cost: f64,
}

pub struct Engine {
    prices: BTreeMap<String, ModelPricing>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::from_json(include_bytes!("models.json"))
            .expect("the checked-in model prices are valid JSON")
    }
}

impl Engine {
    pub fn from_json(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        let mut engine = Self {
            prices: BTreeMap::new(),
        };
        engine.merge_json(bytes)?;
        Ok(engine)
    }

    pub fn merge_json(&mut self, bytes: &[u8]) -> Result<(), serde_json::Error> {
        let prices: BTreeMap<String, ModelPricing> = serde_json::from_slice(bytes)?;
        self.prices.extend(
            prices
                .into_iter()
                .map(|(name, price)| (name.to_lowercase(), price)),
        );
        Ok(())
    }

    pub fn calculate(&self, model: &str, usage: &TokenUsage) -> Option<f64> {
        let model = model.trim().to_lowercase();
        if model.is_empty() {
            return None;
        }
        let price = self.prices.get(&model).or_else(|| {
            self.prices
                .iter()
                .filter(|(key, _)| model.starts_with(key.as_str()) || key.starts_with(&model))
                .max_by(|(a, _), (b, _)| {
                    let a_specific = model.starts_with(a.as_str());
                    let b_specific = model.starts_with(b.as_str());
                    a_specific
                        .cmp(&b_specific)
                        .then_with(|| {
                            if a_specific {
                                a.len().cmp(&b.len())
                            } else {
                                b.len().cmp(&a.len())
                            }
                        })
                        .then_with(|| b.cmp(a))
                })
                .map(|(_, price)| price)
        })?;
        Some(
            usage.input as f64 * price.input_cost_per_token
                + usage.output as f64 * price.output_cost_per_token
                + usage.cache_read as f64 * price.cache_read_input_token_cost
                + usage.cache_write as f64 * price.cache_creation_input_token_cost,
        )
    }
}
