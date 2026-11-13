use std::collections::{HashMap, HashSet};

use serde_json::Value;

/// One of Ladybird's platform features, placed beneath the feature it depends on, as its about:support page lists them.
#[derive(Clone, Debug, PartialEq)]
pub struct PlatformFeatureRow {
    pub id: String,
    /// The id without its parent's, which the parent's row already shows.
    pub name: String,
    pub depth: usize,
    pub state: String,
    pub forced: bool,
    pub rule: Option<String>,
    pub reason: Option<String>,
    pub blocked_by: Option<String>,
    pub has_dependents: bool,
}

/// The rows of a stored `platform_features` field, or `None` when it is not a list of features.
pub fn platform_feature_rows(value: &Value) -> Option<Vec<PlatformFeatureRow>> {
    struct Feature<'a> {
        id: &'a str,
        dependency: Option<&'a str>,
        object: &'a serde_json::Map<String, Value>,
    }

    let text = |object: &serde_json::Map<String, Value>, key: &str| {
        object
            .get(key)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };

    let features = value
        .as_array()?
        .iter()
        .map(|entry| {
            let object = entry.as_object()?;
            object.get("state")?.as_str()?;
            Some(Feature {
                id: object.get("id")?.as_str()?,
                dependency: object.get("dependency").and_then(Value::as_str),
                object,
            })
        })
        .collect::<Option<Vec<_>>>()?;

    let row =
        |feature: &Feature, name: &str, depth: usize, has_dependents: bool| PlatformFeatureRow {
            id: feature.id.to_owned(),
            name: name.to_owned(),
            depth,
            state: text(feature.object, "state").unwrap_or_default(),
            forced: feature
                .object
                .get("forced")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            rule: text(feature.object, "rule"),
            reason: text(feature.object, "reason"),
            blocked_by: text(feature.object, "blocked_by"),
            has_dependents,
        };

    // A feature whose dependency is not in the list is shown at the top, rather than not at all.
    let ids: HashSet<&str> = features.iter().map(|feature| feature.id).collect();
    let mut dependents: HashMap<Option<&str>, Vec<usize>> = HashMap::new();
    for (index, feature) in features.iter().enumerate() {
        let parent = feature
            .dependency
            .filter(|dependency| ids.contains(dependency));
        dependents.entry(parent).or_default().push(index);
    }

    let mut rows = Vec::with_capacity(features.len());
    let mut visited = vec![false; features.len()];
    let mut pending: Vec<(usize, usize)> = dependents
        .get(&None)
        .into_iter()
        .flatten()
        .rev()
        .map(|&index| (index, 0))
        .collect();
    while let Some((index, depth)) = pending.pop() {
        // A list whose dependencies form a cycle still shows each feature once.
        if std::mem::replace(&mut visited[index], true) {
            continue;
        }
        let feature = &features[index];
        let children = dependents.get(&Some(feature.id));
        let name = feature
            .dependency
            .and_then(|dependency| feature.id.strip_prefix(dependency))
            .and_then(|rest| rest.strip_prefix('.'))
            .filter(|_| depth > 0)
            .unwrap_or(feature.id);
        rows.push(row(
            feature,
            name,
            depth,
            children.is_some_and(|children| !children.is_empty()),
        ));
        for &child in children.into_iter().flatten().rev() {
            pending.push((child, depth + 1));
        }
    }

    // Features caught in a cycle never reach the top, so they follow everything else.
    for (index, feature) in features.iter().enumerate() {
        if !visited[index] {
            rows.push(row(feature, feature.id, 0, false));
        }
    }

    Some(rows)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn features_are_listed_beneath_their_dependencies_without_their_prefix() {
        let rows = platform_feature_rows(&json!([
            { "id": "gpu_painting", "state": "Enabled", "reason": "" },
            { "id": "vulkan_video", "dependency": "ycbcr_import", "state": "Enabled" },
            { "id": "ycbcr_import", "dependency": "gpu_painting", "state": "Enabled" },
            {
                "id": "vulkan_video.bundled_ffmpeg",
                "dependency": "vulkan_video",
                "state": "Blocklisted",
                "rule": "a-rule",
                "reason": "Because",
                "forced": true,
            },
        ]))
        .unwrap();

        let outline: Vec<_> = rows
            .iter()
            .map(|row| (row.name.as_str(), row.depth))
            .collect();
        assert_eq!(
            outline,
            [
                ("gpu_painting", 0),
                ("ycbcr_import", 1),
                ("vulkan_video", 2),
                ("bundled_ffmpeg", 3),
            ]
        );
        assert!(rows[0].has_dependents);
        assert!(!rows[3].has_dependents);
        assert_eq!(rows[0].reason, None);
        assert_eq!(rows[3].rule.as_deref(), Some("a-rule"));
        assert_eq!(rows[3].reason.as_deref(), Some("Because"));
        assert!(rows[3].forced);
    }

    #[test]
    fn a_feature_without_its_dependency_is_shown_at_the_top() {
        let rows = platform_feature_rows(&json!([
            { "id": "vulkan_video.bundled_ffmpeg", "dependency": "vulkan_video", "state": "Enabled" },
        ]))
        .unwrap();
        assert_eq!(rows[0].name, "vulkan_video.bundled_ffmpeg");
        assert_eq!(rows[0].depth, 0);
    }

    #[test]
    fn a_cycle_still_shows_every_feature_once() {
        let rows = platform_feature_rows(&json!([
            { "id": "a", "dependency": "b", "state": "Enabled" },
            { "id": "b", "dependency": "a", "state": "Enabled" },
        ]))
        .unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn anything_but_a_list_of_features_is_not_shown_as_one() {
        assert!(platform_feature_rows(&json!("text")).is_none());
        assert!(platform_feature_rows(&json!([{ "id": "a" }])).is_none());
        assert!(platform_feature_rows(&json!([{ "state": "Enabled" }])).is_none());
        assert_eq!(platform_feature_rows(&json!([])), Some(Vec::new()));
    }
}
