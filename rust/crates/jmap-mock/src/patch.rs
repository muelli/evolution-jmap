// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! `PatchObject` application (RFC 8620 §5.3): each key is a JSON-pointer-ish
//! path relative to the object; `null` removes, anything else replaces.

use serde_json::{Map, Value};

/// Apply a patch map to an object. Intermediate objects are created on
/// demand; patching *through* a non-object is an error.
pub(crate) fn apply_patch(target: &mut Value, patch: &Map<String, Value>) -> Result<(), String> {
    let Value::Object(_) = target else {
        return Err("patch target is not an object".to_owned());
    };

    for (path, new_value) in patch {
        let segments: Vec<String> = path
            .split('/')
            .map(|segment| segment.replace("~1", "/").replace("~0", "~"))
            .collect();
        let Some((leaf, parents)) = segments.split_last() else {
            return Err("empty patch path".to_owned());
        };

        let mut current = &mut *target;
        for parent in parents {
            let map = current
                .as_object_mut()
                .ok_or_else(|| format!("{path}: {parent} is not an object"))?;
            current = map
                .entry(parent.clone())
                .or_insert_with(|| Value::Object(Map::new()));
        }
        let map = current
            .as_object_mut()
            .ok_or_else(|| format!("{path}: parent is not an object"))?;
        if new_value.is_null() {
            map.remove(leaf);
        } else {
            map.insert(leaf.clone(), new_value.clone());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::apply_patch;
    use serde_json::json;

    #[test]
    fn top_level_pointer_and_removal() {
        let mut target = json!({"keywords": {"$draft": true}, "subject": "x"});
        let patch = json!({
            "keywords/$seen": true,
            "mailboxIds": {"M2": true},
            "subject": null,
        });
        apply_patch(&mut target, patch.as_object().unwrap()).unwrap();
        assert_eq!(
            target,
            json!({
                "keywords": {"$draft": true, "$seen": true},
                "mailboxIds": {"M2": true},
            })
        );
    }
}
