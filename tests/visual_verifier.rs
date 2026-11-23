use serde_json::Value;
use std::fs;
use std::path::Path;

const SCENES: &[&str] = &[
    "blank",
    "reading",
    "toc",
    "settings",
    "library",
    "light",
    "sepia",
    "dark",
    "charcoal",
    "chapter",
    "address",
    "find",
    "chapter-toc",
    "oled",
    "empty-lib",
];

const PNG_HEADER: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

fn validate_png_file(path: &Path) -> Result<(u32, u32), String> {
    let bytes = fs::read(path).map_err(|e| format!("Failed to read PNG {path:?}: {e}"))?;
    if bytes.len() < 24 {
        return Err(format!("File too short for PNG: {path:?} ({} bytes)", bytes.len()));
    }
    if &bytes[0..8] != PNG_HEADER {
        return Err(format!("Invalid PNG magic header in {path:?}"));
    }

    // Read IHDR dimensions (offset 16-24, big-endian)
    let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);

    if width == 0 || height == 0 {
        return Err(format!("Zero dimensions in PNG {path:?}: {width}x{height}"));
    }

    Ok((width, height))
}

fn walk_accessibility_node(node: &Value, path: &str, violations: &mut Vec<String>) {
    if let Some(bounds) = node.get("bounds").and_then(|b| b.as_array()) {
        if bounds.len() == 4 {
            let x = bounds[0].as_f64().unwrap_or(f64::NAN);
            let y = bounds[1].as_f64().unwrap_or(f64::NAN);
            let w = bounds[2].as_f64().unwrap_or(f64::NAN);
            let h = bounds[3].as_f64().unwrap_or(f64::NAN);

            if x.is_nan() || y.is_nan() || w.is_nan() || h.is_nan() {
                violations.push(format!("{path}: NaN in bounds [{x}, {y}, {w}, {h}]"));
            } else if x.is_infinite() || y.is_infinite() || w.is_infinite() || h.is_infinite() {
                violations.push(format!("{path}: Infinite value in bounds [{x}, {y}, {w}, {h}]"));
            }

            if w < 0.0 || h < 0.0 {
                violations.push(format!("{path}: Inverted or negative dimensions: width={w}, height={h}"));
            }
        } else {
            violations.push(format!("{path}: Bounds array length is {}, expected 4", bounds.len()));
        }
    }

    if let Some(children) = node.get("children").and_then(|c| c.as_array()) {
        for (i, child) in children.iter().enumerate() {
            let child_path = format!("{path}.children[{i}]");
            walk_accessibility_node(child, &child_path, violations);
        }
    }
}

fn validate_json_element_tree(path: &Path) -> Result<(), String> {
    let content = fs::read_to_string(path).map_err(|e| format!("Failed to read JSON {path:?}: {e}"))?;
    let tree: Value = serde_json::from_str(&content).map_err(|e| format!("Invalid JSON in {path:?}: {e}"))?;

    let root_acc = tree.get("accessibility").ok_or_else(|| format!("{path:?}: missing 'accessibility' root"))?;

    // Verify root bounds match 900x720 window
    if let Some(bounds) = root_acc.get("bounds").and_then(|b| b.as_array()) {
        if bounds.len() == 4 {
            let w = bounds[2].as_f64().unwrap_or(0.0);
            let h = bounds[3].as_f64().unwrap_or(0.0);
            assert_eq!(w, 900.0, "Root width mismatch in {path:?}");
            assert_eq!(h, 720.0, "Root height mismatch in {path:?}");
        }
    }

    let mut violations = Vec::new();
    walk_accessibility_node(root_acc, "accessibility", &mut violations);

    if !violations.is_empty() {
        return Err(format!("Violations in {path:?}:\n{}", violations.join("\n")));
    }

    Ok(())
}

#[test]
fn test_verify_all_scenes_snaps_and_trees() {
    let snaps_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("snaps");
    assert!(snaps_dir.exists(), "snaps directory does not exist: {snaps_dir:?}");

    let mut missing_pngs = Vec::new();
    let mut missing_jsons = Vec::new();
    let mut invalid_files = Vec::new();

    for scene in SCENES {
        let png_path = snaps_dir.join(format!("{scene}.png"));
        let json_path = snaps_dir.join(format!("{scene}.json"));

        if !png_path.exists() {
            missing_pngs.push(format!("{scene}.png"));
        } else {
            match validate_png_file(&png_path) {
                Ok((w, h)) => {
                    // Standard scale 2 for 900x720 is 1800x1440
                    assert!(w >= 900 && h >= 720, "Snapshot {png_path:?} dimensions too small: {w}x{h}");
                }
                Err(e) => invalid_files.push(e),
            }
        }

        if !json_path.exists() {
            missing_jsons.push(format!("{scene}.json"));
        } else {
            if let Err(e) = validate_json_element_tree(&json_path) {
                invalid_files.push(e);
            }
        }
    }

    assert!(missing_pngs.is_empty(), "Missing PNG snapshots: {missing_pngs:?}");
    assert!(missing_jsons.is_empty(), "Missing JSON element trees: {missing_jsons:?}");
    assert!(invalid_files.is_empty(), "Validation errors encountered:\n{}", invalid_files.join("\n"));
}
