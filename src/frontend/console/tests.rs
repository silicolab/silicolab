use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use super::{
    Command, ViewKind, execute_console_line, expand_script_variables, parse_command, shell_words,
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

/// `source`/`run` are ordinary subcommands now; a quoted path is de-quoted in
/// exactly one place (the tokenizer) before clap sees it, and `run` is an alias.
#[test]
fn source_and_run_parse_the_script_path_dequoted() {
    let words = shell_words(r#"source "C:\projects\silicolab\reference\sls.sls""#).unwrap();
    match parse_command(&words).unwrap() {
        Command::Source { path } => {
            assert_eq!(
                path,
                PathBuf::from(r"C:\projects\silicolab\reference\sls.sls")
            );
        }
        other => panic!("expected source, got {other:?}"),
    }

    let words = shell_words(r"run C:\tmp\demo.sls").unwrap();
    match parse_command(&words).unwrap() {
        Command::Source { path } => assert_eq!(path, PathBuf::from(r"C:\tmp\demo.sls")),
        other => panic!("expected source via `run` alias, got {other:?}"),
    }
}

/// The tokenizer strips a single wrapping quote pair (double or single) so the
/// script path arrives at clap already de-quoted — backslashes intact.
#[test]
fn tokenizer_strips_one_wrapping_quote_pair() {
    assert_eq!(
        shell_words(r#""C:\tmp\demo.sls""#).unwrap(),
        vec![r"C:\tmp\demo.sls".to_string()]
    );
    assert_eq!(
        shell_words(r#"'C:\tmp\demo.sls'"#).unwrap(),
        vec![r"C:\tmp\demo.sls".to_string()]
    );
}

/// Unbalanced quotes fail at the tokenizer, before any filesystem access — the
/// single place quote handling lives now.
#[test]
fn malformed_script_quotes_fail_before_filesystem_access() {
    assert!(shell_words(r#"source "C:\tmp\demo.sls"#).is_err());
    assert!(shell_words(r#"source C:\tmp\demo.sls""#).is_err());
    assert!(shell_words(r#"source 'C:\tmp\demo.sls"#).is_err());
}

/// The still-deferred domain commands capture their tail verbatim
/// (hyphen-leading values and all) and hand it to the existing parsers. This is
/// the path the GUI console and `.sls` scripts take; the agent heavy-path splits
/// the same string itself.
#[test]
fn deferred_domain_commands_pass_their_tail_through_unparsed() {
    // A leading-hyphen first token and a negative value both survive.
    let qm = shell_words("qm energy --method rhf --charge -1").unwrap();
    match parse_command(&qm).unwrap() {
        Command::Qm { args } => {
            assert_eq!(args, vec!["energy", "--method", "rhf", "--charge", "-1"])
        }
        other => panic!("expected qm pass-through, got {other:?}"),
    }

    // `pack` is an alias of `disorder`; the tail still passes through.
    let pack = shell_words("pack --of active --box 10,10,10").unwrap();
    match parse_command(&pack).unwrap() {
        Command::Disorder { args } => {
            assert_eq!(args, vec!["--of", "active", "--box", "10,10,10"])
        }
        other => panic!("expected disorder via `pack`, got {other:?}"),
    }
}

/// `dock`/`score` are now nested clap: flags become typed fields (negative box
/// coordinates included), so the body and the agent path share one parser.
#[test]
fn dock_flags_parse_into_typed_fields() {
    let dock =
        shell_words("dock --receptor #1 --ligand #2 --center -1,2,3 --exhaustiveness 4").unwrap();
    match parse_command(&dock).unwrap() {
        Command::Dock(args) => {
            assert_eq!(args.receptor.as_deref(), Some("#1"));
            assert_eq!(args.ligand.as_deref(), Some("#2"));
            assert_eq!(args.center.as_deref(), Some("-1,2,3"));
            assert_eq!(args.exhaustiveness, Some(4));
            assert_eq!(args.size, None);
        }
        other => panic!("expected dock, got {other:?}"),
    }

    match parse_command(&shell_words("score --receptor active --ligand #2").unwrap()).unwrap() {
        Command::Score(args) => assert_eq!(args.receptor.as_deref(), Some("active")),
        other => panic!("expected score, got {other:?}"),
    }
}

/// `--global` is accepted both after the subcommand keyword and before it.
#[test]
fn global_flag_is_accepted_in_either_position() {
    for line in [
        "view background white --global",
        "view --global background white",
    ] {
        match parse_command(&shell_words(line).unwrap()).unwrap() {
            Command::View(args) => {
                assert!(args.global.global, "`--global` not seen in `{line}`");
                assert!(matches!(args.kind, ViewKind::Background { .. }));
            }
            other => panic!("expected view, got {other:?}"),
        }
    }

    // Without it, `global` is false (and `focus` aliases `activate`).
    match parse_command(&shell_words("surface style mesh").unwrap()).unwrap() {
        Command::Surface(args) => assert!(!args.global.global),
        other => panic!("expected surface, got {other:?}"),
    }
}

/// A quoted, spaced path is one token by the time clap sees it.
#[test]
fn quoted_spaced_path_is_a_single_open_argument() {
    let words = shell_words(r#"open "C:\some path\x.pdb""#).unwrap();
    match parse_command(&words).unwrap() {
        Command::Open { path } => assert_eq!(path, r"C:\some path\x.pdb"),
        other => panic!("expected open, got {other:?}"),
    }
}

/// The bare `*.sls` shortcut only fires for a single whitespace-free `.sls`
/// token, never for a multi-word line.
#[test]
fn bare_script_path_shortcut_recognition() {
    assert!(super::looks_like_script_path("foo.sls"));
    assert!(super::looks_like_script_path(r"C:\scripts\demo.SLS"));
    assert!(!super::looks_like_script_path("foo.pdb"));
    assert!(!super::looks_like_script_path("source foo.sls"));
}

/// Drift guard: every top-level command in the clap tree must be documented in
/// the assistant's `command_catalog()`, except the scripting/meta plumbing that
/// is deliberately not an assistant action.
#[test]
fn every_command_is_catalogued_or_exempt() {
    // - source/run: scripting plumbing, not an assistant action.
    // - help: meta.
    // - disorder/pack: GUI/CLI-only today; not yet surfaced to the assistant.
    //   TODO(catalog): add `disorder` to command_catalog() if/when the assistant
    //   should drive packing, then drop it from this list.
    const EXEMPT: &[&str] = &["source", "disorder", "help"];
    let catalog = super::command_catalog();
    for name in super::top_level_command_names() {
        if EXEMPT.contains(&name.as_str()) {
            continue;
        }
        assert!(
            catalog.contains(name.as_str()),
            "top-level command `{name}` is missing from command_catalog(); \
             document it for the assistant or add it to the EXEMPT list"
        );
    }
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
    let words =
        shell_words("fetch 4hhb --db https://example.org/pdb --dir tmp/structures").unwrap();
    match parse_command(&words).unwrap() {
        Command::Fetch { id, db, dir } => {
            assert_eq!(id, "4hhb");
            assert_eq!(db.as_deref(), Some("https://example.org/pdb"));
            assert_eq!(dir.unwrap(), PathBuf::from("tmp/structures"));
        }
        other => panic!("expected fetch, got {other:?}"),
    }
}

#[test]
fn fetch_command_args_reject_unknown_flags() {
    let words = shell_words("fetch 4hhb --oops").unwrap();
    assert!(
        parse_command(&words).is_err(),
        "an unknown flag should fail to parse"
    );
}

/// `help` is now clap-rendered, and an unknown command still comes back through
/// the `Err` channel (shown as `command failed: ...`) rather than exiting.
#[test]
fn help_renders_and_unknown_command_errors() {
    let mut state = AppState::scratch(Default::default(), Vec::new());

    let help = execute_console_line(&mut state, "help").expect("help should render");
    for expected in ["open", "view", "dock"] {
        assert!(
            help.contains(expected),
            "help missing `{expected}`:\n{help}"
        );
    }

    assert!(
        execute_console_line(&mut state, "definitely-not-a-command").is_err(),
        "an unknown command must surface as an error, not exit"
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
fn sketch_rejects_invalid_smiles_with_a_hint() {
    let mut state = AppState::scratch(Default::default(), Vec::new());
    // `HH` is not valid SMILES (hydrogen must be bracketed); the user hit this
    // trying to build H2. The error should point at the explicit-atom form.
    let error = execute_console_line(&mut state, "sketch HH").unwrap_err();
    let text = error.to_string();
    assert!(text.contains("could not sketch `HH`"), "error: {text}");
    assert!(
        text.contains("[H][H]"),
        "error should hint the H2 SMILES: {text}"
    );
}

/// `sketch` parses the SMILES positionally and an optional `--name`; without the
/// flag the entry name later defaults to the SMILES text.
#[test]
fn sketch_parses_optional_name_flag() {
    match parse_command(&shell_words("sketch CCO --name ethanol").unwrap()).unwrap() {
        Command::Sketch { smiles, name } => {
            assert_eq!(smiles, "CCO");
            assert_eq!(name.as_deref(), Some("ethanol"));
        }
        other => panic!("expected sketch, got {other:?}"),
    }

    match parse_command(&shell_words("sketch CCO").unwrap()).unwrap() {
        Command::Sketch { smiles, name } => {
            assert_eq!(smiles, "CCO");
            assert_eq!(name, None);
        }
        other => panic!("expected sketch, got {other:?}"),
    }
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
