use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use super::{
    execute_console_line, expand_script_variables, normalize_script_path, parse_fetch_command_args,
    script_source_path,
};
use crate::frontend::{
    LightPreset, SurfaceStyle, ViewportVisualState,
    state::{AppState, AtomStyle},
};
use eframe::egui::Color32;

const CONSOLE_TEST_PDB: &str = "\
ATOM      1  N   GLY A   1       0.000   0.000   0.000  1.00  0.00           N
ATOM      2  CA  GLY A   1       1.450   0.000   0.000  1.00  0.00           C
ATOM      3  N   ALA A   2       2.900   0.000   0.000  1.00  0.00           N
END
";

fn write_console_fixture(name: &str, contents: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    let dir = std::env::temp_dir().join("silicolab_console_tests");
    fs::create_dir_all(&dir).expect("fixture dir");
    let path = dir.join(format!("{name}_{nonce}.pdb"));
    fs::write(&path, contents).expect("fixture file");
    path
}

fn open_fixture_command(path: &Path) -> String {
    format!("open {}", path.display())
}

#[test]
fn source_and_run_take_the_rest_of_the_line_as_script_path() {
    let path = r#""C:\projects\silicolab\reference\sls.sls""#;
    assert_eq!(script_source_path(&format!("source {path}")), Some(path));
    assert_eq!(
        script_source_path("run C:\\tmp\\demo.sls"),
        Some("C:\\tmp\\demo.sls")
    );
}

#[test]
fn script_paths_allow_one_wrapping_quote_pair() {
    assert_eq!(
        normalize_script_path(r#""C:\tmp\demo.sls""#).unwrap(),
        r"C:\tmp\demo.sls"
    );
    assert_eq!(
        normalize_script_path(r#"'C:\tmp\demo.sls'"#).unwrap(),
        r"C:\tmp\demo.sls"
    );
}

#[test]
fn malformed_script_quotes_fail_before_filesystem_access() {
    assert!(normalize_script_path(r#""C:\tmp\demo.sls"#).is_err());
    assert!(normalize_script_path(r#"C:\tmp\demo.sls""#).is_err());
    assert!(normalize_script_path(r#"'C:\tmp\demo.sls"#).is_err());
}

#[test]
fn expands_script_variables_with_optional_defaults() {
    let mut variables = BTreeMap::new();
    variables.insert("width".to_string(), "900".to_string());

    assert_eq!(
        expand_script_variables("view size ${width} ${height:-500}", &variables).unwrap(),
        "view size 900 500"
    );
}

#[test]
fn missing_script_variables_fail_without_default() {
    let error = expand_script_variables("save image ${output}", &BTreeMap::new())
        .expect_err("missing output should fail");
    assert!(
        error
            .to_string()
            .contains("missing script variable `output`")
    );
}

#[test]
fn fetch_command_args_support_db_and_dir_flags() {
    let parsed = parse_fetch_command_args(&[
        "4hhb".to_string(),
        "--db".to_string(),
        "https://example.org/pdb".to_string(),
        "--dir".to_string(),
        "tmp/structures".to_string(),
    ])
    .unwrap();

    assert_eq!(parsed.id, "4hhb");
    assert_eq!(parsed.base_url, "https://example.org/pdb");
    assert_eq!(parsed.dir.unwrap(), PathBuf::from("tmp/structures"));
}

#[test]
fn fetch_command_args_reject_unknown_flags() {
    let error = parse_fetch_command_args(&["4hhb".to_string(), "--oops".to_string()])
        .expect_err("unknown flags should fail");
    assert!(
        error
            .to_string()
            .contains("unknown flag `--oops` for fetch")
    );
}

#[test]
fn render_commands_are_scoped_to_the_active_entry() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let fixture = write_console_fixture("render_scope", CONSOLE_TEST_PDB);

    execute_console_line(&mut state, &open_fixture_command(&fixture)).unwrap();
    let first_entry_id = state.entries.active_entry_id().unwrap();
    execute_console_line(&mut state, "surface style mesh").unwrap();
    execute_console_line(&mut state, "surface chain A").unwrap();
    // `sphere` differs from the protein smart default (cartoon), so it is
    // observable that the style is scoped to this entry.
    execute_console_line(&mut state, "representation sphere").unwrap();

    execute_console_line(&mut state, &open_fixture_command(&fixture)).unwrap();
    let second_entry_id = state.entries.active_entry_id().unwrap();
    assert_ne!(first_entry_id, second_entry_id);
    // The fresh entry resolves protein → cartoon from the category tiers,
    // not the first entry's per-atom sphere styling.
    assert_eq!(
        state.ui.viewport.resolved_atom_style(state.structure(), 0),
        AtomStyle::Cartoon
    );
    assert_eq!(state.ui.viewport.surface.style, SurfaceStyle::Mesh);
    assert!(state.ui.viewport.surface.chains.is_empty());

    state.save_viewport_for_active_entry();
    state.entries.activate_entry(first_entry_id);
    state.load_viewport_for_active_entry();
    assert_eq!(
        state.ui.viewport.resolved_atom_style(state.structure(), 0),
        AtomStyle::Sphere
    );
    assert_eq!(state.ui.viewport.surface.style, SurfaceStyle::Mesh);
    assert!(state.ui.viewport.surface.chains.contains(&'A'));
}

#[test]
fn global_render_commands_update_project_defaults() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let fixture = write_console_fixture("render_global", CONSOLE_TEST_PDB);

    execute_console_line(&mut state, &open_fixture_command(&fixture)).unwrap();
    execute_console_line(&mut state, "surface style mesh --global").unwrap();
    execute_console_line(&mut state, "representation sphere --global").unwrap();

    execute_console_line(&mut state, &open_fixture_command(&fixture)).unwrap();
    // Global surface settings propagate to new entries. The `representation`
    // command is per-entry atom-level, so the fresh entry still resolves its
    // protein to cartoon via the category tiers.
    assert_eq!(state.ui.viewport.surface.style, SurfaceStyle::Mesh);
    assert_eq!(
        state.ui.viewport.resolved_atom_style(state.structure(), 0),
        AtomStyle::Cartoon
    );
}

#[test]
fn view_script_export_roundtrips() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let fixture = write_console_fixture("view_export", CONSOLE_TEST_PDB);
    execute_console_line(&mut state, &open_fixture_command(&fixture)).unwrap();

    for line in [
        "view background #102030",
        "view cell off",
        "view light studio",
        "cartoon helix --width 3 --thickness 0.4",
        "color chain A #ff8800",
        "surface style mesh",
        "surface transparency 50",
        "surface chain A",
        "show ions --within 4",
    ] {
        execute_console_line(&mut state, line).unwrap();
    }

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let script_path = std::env::temp_dir()
        .join("silicolab_console_tests")
        .join(format!("view_{nonce}.sls"));
    execute_console_line(&mut state, &format!("save view {}", script_path.display())).unwrap();

    // A fresh entry resets the viewport to defaults...
    execute_console_line(&mut state, &open_fixture_command(&fixture)).unwrap();
    assert_eq!(
        state.ui.viewport.background_color,
        ViewportVisualState::default().background_color
    );

    // ...and replaying the exported script reproduces every setting.
    execute_console_line(&mut state, &format!("run {}", script_path.display())).unwrap();
    let viewport = &state.ui.viewport;
    assert_eq!(
        viewport.background_color,
        Color32::from_rgb(0x10, 0x20, 0x30)
    );
    assert!(!viewport.show_cell);
    assert_eq!(viewport.lighting.preset, LightPreset::Studio);
    assert!((viewport.cartoon.helix.width - 3.0).abs() < 1e-4);
    assert!((viewport.cartoon.helix.thickness - 0.4).abs() < 1e-4);
    assert_eq!(
        viewport.chain_colors.get(&'A'),
        Some(&Color32::from_rgb(0xff, 0x88, 0x00))
    );
    assert_eq!(viewport.surface.style, SurfaceStyle::Mesh);
    assert!((viewport.surface.transparency - 0.5).abs() < 1e-4);
    assert!(viewport.surface.chains.contains(&'A'));
    assert_eq!(viewport.ions.show_within, Some(4.0));
}

#[test]
fn activate_switches_the_active_entry_by_id() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let fixture = write_console_fixture("activate", CONSOLE_TEST_PDB);

    execute_console_line(&mut state, &open_fixture_command(&fixture)).unwrap();
    let first = state.entries.active_entry_id().unwrap();
    execute_console_line(&mut state, &open_fixture_command(&fixture)).unwrap();
    let second = state.entries.active_entry_id().unwrap();
    assert_ne!(first, second);

    // `#id` switches back to the earlier entry; the message echoes the id.
    let message = execute_console_line(&mut state, &format!("activate #{first}")).unwrap();
    assert_eq!(state.entries.active_entry_id(), Some(first));
    assert!(message.contains(&format!("#{first}")), "message: {message}");

    // A bare integer is also accepted, and `focus` is an alias.
    execute_console_line(&mut state, &format!("activate {second}")).unwrap();
    assert_eq!(state.entries.active_entry_id(), Some(second));
    execute_console_line(&mut state, &format!("focus #{first}")).unwrap();
    assert_eq!(state.entries.active_entry_id(), Some(first));
}

#[test]
fn activate_reports_unresolvable_references() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    let fixture = write_console_fixture("activate_err", CONSOLE_TEST_PDB);

    // Nothing open yet.
    let empty = execute_console_line(&mut state, "activate #1").unwrap_err();
    assert!(empty.to_string().contains("no entries are open"));

    execute_console_line(&mut state, &open_fixture_command(&fixture)).unwrap();
    execute_console_line(&mut state, &open_fixture_command(&fixture)).unwrap();

    // An id with no entry, and a missing argument, both error.
    let bad_id = execute_console_line(&mut state, "activate #99").unwrap_err();
    assert!(bad_id.to_string().contains("no open entry with id #99"));
    assert!(execute_console_line(&mut state, "activate").is_err());

    // The two fixtures import under the same name, so a name reference is
    // ambiguous and must be disambiguated by id.
    let duplicate_name = state.entries.active_entry().unwrap().name.clone();
    let ambiguous =
        execute_console_line(&mut state, &format!("activate {duplicate_name}")).unwrap_err();
    assert!(
        ambiguous.to_string().contains("matches 2 entries"),
        "error: {ambiguous}"
    );
}
