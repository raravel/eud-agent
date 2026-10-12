use std::cell::Cell;

use crate::e2_import::resources::{
    resolve_copy_plan, E2ResourceResolveError, E2ResourceResolveRequest,
};
use crate::e2_import::E2SourceKind;

use super::{map_only_document, test_fs, FixtureRoot};

pub(super) fn run() {
    let fixture = FixtureRoot::new("mid-read-cancel");
    let source_bytes = vec![b'X'; 64 * 1024 + 1];
    let source = fixture.write("project.e2s", &source_bytes);
    let sentinel = fixture.write("destination/sentinel.txt", b"preserve");
    let document = map_only_document(E2SourceKind::E2s, "basemap.scx");
    let checks = Cell::new(0_u32);

    let result = resolve_copy_plan(
        E2ResourceResolveRequest {
            source_path: &source,
            document: &document,
            relinks: &[],
            expected_fingerprint: None,
        },
        || {
            let next = checks.get() + 1;
            checks.set(next);
            next == 3
        },
    );

    assert!(matches!(result, Err(E2ResourceResolveError::Cancelled)));
    assert_eq!(
        checks.get(),
        3,
        "the third check occurs only after one 64 KiB chunk was appended"
    );
    assert_eq!(test_fs::read(&source).unwrap(), source_bytes);
    assert_eq!(test_fs::read(sentinel).unwrap(), b"preserve");
}
