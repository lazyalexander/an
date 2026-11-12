use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AskReason {
    EmptyDescription,
    CountMismatch,
    UnknownObject,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairError {
    Ask(AskReason),
    InvalidName,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedLayer {
    pub name: String,
    pub color: String,
    pub component: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderPlan {
    pub layers: Vec<PlannedLayer>,
}

const COLORS: &[(&str, &str)] = &[
    ("黄铜", "#d4a017"),
    ("青绿", "#148f77"),
    ("红色", "#c0392b"),
    ("蓝色", "#2471a3"),
    ("绿色", "#1e8449"),
    ("黄色", "#f4d03f"),
    ("黑色", "#111111"),
    ("白色", "#f7f7f7"),
    ("红", "#c0392b"),
    ("蓝", "#2471a3"),
    ("绿", "#1e8449"),
    ("黄", "#f4d03f"),
    ("黑", "#111111"),
    ("白", "#f7f7f7"),
    ("钢", "#b0b7bf"),
];

pub fn pair(description: &str, component_count: usize) -> Result<RenderPlan, PairError> {
    let description = description.trim();
    if description.is_empty() {
        return Err(PairError::Ask(AskReason::EmptyDescription));
    }
    let clauses: Vec<&str> = description
        .split(['，', '、', ',', '；', ';'])
        .map(str::trim)
        .filter(|clause| !clause.is_empty())
        .collect();
    if clauses.len() != component_count {
        return Err(PairError::Ask(AskReason::CountMismatch));
    }
    let mut layers = Vec::with_capacity(clauses.len());
    for (component, clause) in clauses.into_iter().enumerate() {
        let (name, color) = name_and_color(clause)?;
        layers.push(PlannedLayer {
            name,
            color,
            component,
        });
    }
    Ok(RenderPlan { layers })
}

pub fn validate_plan(plan: &RenderPlan, component_count: usize) -> Result<(), String> {
    for layer in &plan.layers {
        if layer.name.is_empty() || layer.name.contains('/') {
            return Err("layer name is empty or contains a slash".into());
        }
        if !is_color(&layer.color) {
            return Err(format!("color {} is not #rrggbb", layer.color));
        }
        if layer.component >= component_count {
            return Err(format!(
                "component {} is outside this stroke diff",
                layer.component
            ));
        }
    }
    Ok(())
}

fn name_and_color(clause: &str) -> Result<(String, String), PairError> {
    let (name, color) = match find_color(clause) {
        Some((pos, word, hex)) => {
            let name = format!("{}{}", &clause[..pos], &clause[pos + word.len()..]);
            let name = name.trim().trim_matches('的').trim().to_string();
            (name, hex.to_string())
        }
        None => (clause.trim().to_string(), "#888888".to_string()),
    };
    if name.is_empty() || name.contains('/') {
        return Err(PairError::InvalidName);
    }
    Ok((name, color))
}

fn find_color(clause: &str) -> Option<(usize, &str, &str)> {
    let mut best: Option<(usize, &str, &str)> = None;
    for (word, hex) in COLORS {
        if let Some(pos) = clause.find(word) {
            let take = match best {
                None => true,
                Some((at, previous, _)) => pos < at || (pos == at && word.len() > previous.len()),
            };
            if take {
                best = Some((pos, word, hex));
            }
        }
    }
    best
}

fn is_color(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 7 && bytes[0] == b'#' && bytes[1..].iter().all(|b| b.is_ascii_hexdigit())
}

pub fn parse_color(value: &str) -> Option<[u8; 3]> {
    if !is_color(value) {
        return None;
    }
    let value = value.trim_start_matches('#');
    Some([
        u8::from_str_radix(&value[0..2], 16).ok()?,
        u8::from_str_radix(&value[2..4], 16).ok()?,
        u8::from_str_radix(&value[4..6], 16).ok()?,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_two_chinese_clauses_with_colors() {
        let plan = pair("红色轮子，黄铜握把", 2).unwrap();
        assert_eq!(plan.layers[0].name, "轮子");
        assert_eq!(plan.layers[0].color, "#c0392b");
        assert_eq!(plan.layers[0].component, 0);
        assert_eq!(plan.layers[1].name, "握把");
        assert_eq!(plan.layers[1].color, "#d4a017");
        assert_eq!(plan.layers[1].component, 1);
    }

    #[test]
    fn splits_on_comma_and_enumeration_and_drops_empty_clauses() {
        assert_eq!(pair("红色轮子,黄铜握把", 2).unwrap().layers.len(), 2);
        assert_eq!(pair("红色轮子、黄铜握把", 2).unwrap().layers.len(), 2);
        assert_eq!(pair("红色轮子，，黄铜握把", 2).unwrap().layers.len(), 2);
    }

    #[test]
    fn rejects_a_count_mismatch_and_an_empty_sentence() {
        assert_eq!(
            pair("红色轮子", 2),
            Err(PairError::Ask(AskReason::CountMismatch))
        );
        assert_eq!(
            pair("红色轮子，黄铜握把", 1),
            Err(PairError::Ask(AskReason::CountMismatch))
        );
        assert_eq!(
            pair("   ", 1),
            Err(PairError::Ask(AskReason::EmptyDescription))
        );
    }

    #[test]
    fn unknown_color_keeps_the_clause_and_uses_gray() {
        let plan = pair("玻璃球", 1).unwrap();
        assert_eq!(plan.layers[0].name, "玻璃球");
        assert_eq!(plan.layers[0].color, "#888888");
    }

    #[test]
    fn rejects_an_empty_name_or_a_slash() {
        assert_eq!(pair("红色", 1), Err(PairError::InvalidName));
        assert_eq!(pair("红/轮子", 1), Err(PairError::InvalidName));
    }
}
