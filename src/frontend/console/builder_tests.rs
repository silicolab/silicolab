//! Parse tests for `polymer build`, `forcefield` and `membrane`; the older builder commands stay in `tests.rs`.

use super::{Command, ForcefieldAction, MembraneAction, PolymerAction, parse_command, shell_words};

#[test]
fn polymer_build_parses_units_and_flags() {
    let line = "polymer build [*]CC[*] [*]CC([*])c1ccccc1 --dp 12 --sequence AB --seed 3 --no-relax --name ps";
    match parse_command(&shell_words(line).unwrap()).unwrap() {
        Command::Polymer {
            action: PolymerAction::Build(args),
        } => {
            assert_eq!(args.units, ["[*]CC[*]", "[*]CC([*])c1ccccc1"]);
            assert_eq!(args.dp, 12);
            assert_eq!(args.sequence.as_deref(), Some("AB"));
            assert_eq!(args.seed, 3);
            assert!(args.no_relax);
            assert_eq!(args.name.as_deref(), Some("ps"));
        }
        other => panic!("expected polymer build, got {other:?}"),
    }
    match parse_command(&shell_words("polymer build [*]CC[*]").unwrap()).unwrap() {
        Command::Polymer {
            action: PolymerAction::Build(args),
        } => {
            assert_eq!(args.dp, 10);
            assert!(!args.no_relax);
        }
        other => panic!("expected polymer build, got {other:?}"),
    }
    assert!(parse_command(&shell_words("polymer build").unwrap()).is_err());
}

#[test]
fn forcefield_type_parses_entry_and_flags() {
    let line = "forcefield type #3 --charge -1 --method gasteiger --atom-types gaff --name acetate --timeout 5";
    match parse_command(&shell_words(line).unwrap()).unwrap() {
        Command::Forcefield {
            action: ForcefieldAction::Type(args),
        } => {
            assert_eq!(args.entry, "#3");
            assert_eq!(args.charge, Some(-1));
            assert_eq!(args.method, "gasteiger");
            assert_eq!(args.atom_types, "gaff");
            assert_eq!(args.name.as_deref(), Some("acetate"));
            assert_eq!(args.timeout, 5);
        }
        other => panic!("expected forcefield type, got {other:?}"),
    }
    match parse_command(&shell_words("ff type").unwrap()).unwrap() {
        Command::Forcefield {
            action: ForcefieldAction::Type(args),
        } => {
            assert_eq!(args.entry, "active");
            assert_eq!(args.charge, None);
            assert_eq!(args.method, "bcc");
            assert_eq!(args.atom_types, "gaff2");
            assert_eq!(args.timeout, 30);
        }
        other => panic!("expected forcefield type, got {other:?}"),
    }
    assert!(matches!(
        parse_command(&shell_words("forcefield list").unwrap()).unwrap(),
        Command::Forcefield {
            action: ForcefieldAction::List
        }
    ));
}

#[test]
fn membrane_commands_parse_their_flags() {
    let line = "membrane diffusion runs/md/md.xtc --entry #3 --select BEN --max-lag 200 --fit 0.1,0.9 --out msd.csv";
    match parse_command(&shell_words(line).unwrap()).unwrap() {
        Command::Membrane {
            action: MembraneAction::Diffusion(args),
        } => {
            assert_eq!(
                args.trajectory.as_deref(),
                Some(std::path::Path::new("runs/md/md.xtc"))
            );
            assert_eq!(args.entry, "#3");
            assert_eq!(args.select, "BEN");
            assert_eq!(args.max_lag, Some(200.0));
            assert_eq!(args.fit, "0.1,0.9");
            assert_eq!(args.out.as_deref(), Some(std::path::Path::new("msd.csv")));
        }
        other => panic!("expected membrane diffusion, got {other:?}"),
    }
    match parse_command(&shell_words("membrane free-volume --probe 1.3 --frames 5").unwrap())
        .unwrap()
    {
        Command::Membrane {
            action: MembraneAction::FreeVolume(args),
        } => {
            assert_eq!(args.entry, "active");
            assert_eq!(args.probe, 1.3);
            assert_eq!(args.grid, 0.5);
            assert!(args.trajectory.is_none());
            assert_eq!(args.frames, Some(5));
        }
        other => panic!("expected membrane free-volume, got {other:?}"),
    }
}
