//! Parse tests for `polymer build`; the older builder commands stay in `tests.rs`.

use super::{Command, PolymerAction, parse_command, shell_words};

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
