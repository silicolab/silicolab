use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use super::*;

fn mock_server(
    expected_requests: usize,
    response: impl Fn(&str) -> (&'static str, &'static str) + Send + 'static,
) -> (String, Arc<Mutex<Vec<String>>>, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = Arc::clone(&requests);
    let handle = std::thread::spawn(move || {
        for _ in 0..expected_requests {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0_u8; 8192];
            let size = stream.read(&mut buffer).unwrap();
            let request = String::from_utf8_lossy(&buffer[..size]).to_string();
            captured.lock().unwrap().push(request.clone());
            let (status, body) = response(&request);
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
    });
    (base, requests, handle)
}

#[test]
fn formula_is_normalized_for_cod() {
    assert_eq!(normalize_formula_for_cod("C12H10").unwrap(), "C12 H10");
    assert_eq!(normalize_formula_for_cod("Na Cl").unwrap(), "Na Cl");
    assert!(normalize_formula_for_cod("biphenyl").is_err());
}

#[test]
fn path_segments_are_percent_encoded() {
    assert_eq!(encode_path_segment("C/C=C\\C"), "C%2FC%3DC%5CC");
    assert_eq!(encode_path_segment("sodium chloride"), "sodium%20chloride");
}

#[test]
fn exact_room_temperature_candidate_sorts_first() {
    let candidate =
        |id: &str, exact: bool, temperature: Option<f64>, r: Option<f64>| CrystalCandidate {
            cod_id: id.to_string(),
            revision: None,
            cid: None,
            smiles: None,
            name: id.to_string(),
            formula: String::new(),
            temperature_k: temperature,
            space_group: None,
            r_factor: r,
            doi: None,
            flags: Vec::new(),
            exact_formula: exact,
            warnings: Vec::new(),
        };
    let mut values = [
        candidate("2", false, Some(295.0), Some(0.02)),
        candidate("1", true, Some(295.0), Some(0.04)),
        candidate("3", true, Some(100.0), Some(0.03)),
    ];
    values.sort_by(compare_candidates);
    assert_eq!(values[0].cod_id, "1");
}

#[test]
fn name_search_resolves_pubchem_and_queries_cod() {
    let (base, requests, server) = mock_server(4, |request| {
        if request.contains("/compound/name/") {
            return (
                "200 OK",
                r#"{"PropertyTable":{"Properties":[{"CID":5234,"Title":"Sodium chloride","MolecularFormula":"NaCl","SMILES":"[Na+].[Cl-]"}]}}"#,
            );
        }
        if request.contains("format=count") && request.contains("formula=") {
            return ("200 OK", "1");
        }
        if request.contains("format=count") && request.contains("smarts=") {
            return ("200 OK", "0");
        }
        (
            "200 OK",
            r#"[{"file":"1234567","commonname":"Halite","formula":"Na Cl","celltemp":"295","sg":"F m -3 m","Robs":"0.021","doi":"10.1/example","flags":"","svnrevision":"280001"}]"#,
        )
    });
    let endpoints = SearchEndpoints {
        pubchem_base_url: format!("{base}/rest/pug"),
        cod_base_url: format!("{base}/cod"),
    };
    let result = search_structures_with_endpoints(
        &StructureQuery::new("sodium chloride", QueryKind::Name),
        &endpoints,
    )
    .unwrap();
    server.join().unwrap();

    assert_eq!(result.resolved.unwrap().cid, 5234);
    assert_eq!(result.crystals[0].cod_id, "1234567");
    assert_eq!(result.crystals[0].revision.as_deref(), Some("280001"));
    let requests = requests.lock().unwrap().join("\n");
    assert!(requests.contains("/compound/name/sodium%20chloride/"));
    assert!(requests.contains("formula=Na%20Cl") || requests.contains("formula=Na+Cl"));
    assert!(requests.contains("smarts="));
}

#[test]
fn fixed_revision_fetch_is_cached_after_validation() {
    let cif = "data_mock\n_cell_length_a 5\n_cell_length_b 5\n_cell_length_c 5\n_cell_angle_alpha 90\n_cell_angle_beta 90\n_cell_angle_gamma 90\nloop_\n_atom_site_label\n_atom_site_fract_x\n_atom_site_fract_y\n_atom_site_fract_z\nC1 0 0 0\n";
    let leaked_cif: &'static str = Box::leak(cif.to_string().into_boxed_str());
    let (base, requests, server) = mock_server(1, move |_| ("200 OK", leaked_cif));
    let candidate = CrystalCandidate {
        cod_id: "1234567".to_string(),
        revision: Some("42".to_string()),
        cid: None,
        smiles: None,
        name: "Mock crystal".to_string(),
        formula: "C".to_string(),
        temperature_k: Some(295.0),
        space_group: Some("P 1".to_string()),
        r_factor: Some(0.02),
        doi: None,
        flags: Vec::new(),
        exact_formula: true,
        warnings: Vec::new(),
    };
    let root = std::env::temp_dir().join(format!("silicolab-cod-test-{}", uuid::Uuid::new_v4()));
    let first = fetch_cod_with_base_url("carbon", &candidate, &root, &base).unwrap();
    server.join().unwrap();
    let second = fetch_cod_with_base_url("carbon", &candidate, &root, &base).unwrap();

    assert!(first.downloaded);
    assert!(!second.downloaded);
    assert_eq!(second.structure.title, "Mock crystal");
    assert!(second.path.ends_with("cod/1234567@42.cif"));
    assert_eq!(requests.lock().unwrap().len(), 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn overbroad_cod_query_stops_after_count_preflight() {
    let (base, requests, server) = mock_server(1, |_| ("200 OK", "251"));
    let result = search_structures_with_endpoints(
        &StructureQuery::new("C", QueryKind::Formula),
        &SearchEndpoints {
            pubchem_base_url: format!("{base}/rest/pug"),
            cod_base_url: format!("{base}/cod"),
        },
    );
    server.join().unwrap();

    assert!(result.unwrap_err().to_string().contains("safety cap"));
    assert_eq!(requests.lock().unwrap().len(), 1);
}

#[test]
fn html_cif_response_is_rejected_without_populating_cache() {
    let (base, _, server) = mock_server(1, |_| {
        ("200 OK", "<!doctype html><title>upstream error</title>")
    });
    let candidate = CrystalCandidate {
        cod_id: "1234567".to_string(),
        revision: Some("42".to_string()),
        cid: None,
        smiles: None,
        name: "Bad response".to_string(),
        formula: "C".to_string(),
        temperature_k: None,
        space_group: Some("P 1".to_string()),
        r_factor: None,
        doi: None,
        flags: Vec::new(),
        exact_formula: true,
        warnings: Vec::new(),
    };
    let root = std::env::temp_dir().join(format!("silicolab-cod-test-{}", uuid::Uuid::new_v4()));
    let result = fetch_cod_with_base_url("carbon", &candidate, &root, &base);
    server.join().unwrap();

    assert!(result.unwrap_err().to_string().contains("non-CIF"));
    assert!(!root.join("cod/1234567@42.cif").exists());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
#[ignore = "requires PubChem and COD network access"]
fn live_name_search_returns_biphenyl_metadata() {
    let mut query = StructureQuery::new("biphenyl", QueryKind::Name);
    query.limit = 5;
    let result = search_structures(&query).unwrap();
    let resolved = result.resolved.expect("PubChem resolution");
    assert_eq!(resolved.formula, "C12H10");
    assert!(!resolved.smiles.is_empty());
    assert!(!result.crystals.is_empty());
    assert!(result.crystals.iter().all(|candidate| {
        candidate.cod_id.len() == 7
            && candidate.cod_id.chars().all(|value| value.is_ascii_digit())
            && candidate.cid == Some(resolved.cid)
            && candidate.smiles.as_deref() == Some(resolved.smiles.as_str())
    }));
}
