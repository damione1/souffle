//! Shared SQL identifiers must come from the contract, even when a constant's
//! value changes. Reject local identifiers rather than grepping today's names.

fn sql_literals(source: &str) -> Vec<&str> {
    let bytes = source.as_bytes();
    let mut literals = Vec::new();
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor..].starts_with(b"//") {
            cursor += source[cursor..].find('\n').unwrap_or(bytes.len() - cursor);
        } else if bytes[cursor] == b'"' {
            let start = cursor + 1;
            cursor = start;
            while cursor < bytes.len() && bytes[cursor] != b'"' {
                cursor += if bytes[cursor] == b'\\' { 2 } else { 1 };
            }
            let literal = &source[start..cursor.min(bytes.len())];
            let first = literal.split_whitespace().next().unwrap_or_default();
            if [
                "SELECT", "INSERT", "DELETE", "UPDATE", "CREATE", "DROP", "ALTER",
            ]
            .contains(&first)
            {
                literals.push(literal);
            }
            cursor += 1;
        } else {
            cursor += 1;
        }
    }
    literals
}

fn local_identifiers(sql: &str) -> Vec<&str> {
    let mut identifiers = Vec::new();
    // SQL data literals and Rust format captures are not local identifiers.
    let mut quoted = false;
    let mut captured = false;
    for part in sql.split_inclusive(['\'', '{', '}']) {
        if !quoted && !captured {
            identifiers.extend(
                part.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                    .filter(|word| word.starts_with(|c: char| c.is_ascii_lowercase()))
                    .filter(|word| {
                        ![
                            "m",
                            "s",
                            "ts",
                            "excluded",
                            "snippet",
                            "julianday",
                            "fts5",
                            "idx_segments_meeting",
                        ]
                        .contains(word)
                            && !(sql.contains("FROM sqlite_master")
                                && ["sqlite_master", "type", "name"].contains(word))
                    }),
            );
        }
        match part.as_bytes().last() {
            Some(b'\'') => quoted = !quoted,
            Some(b'{') if !quoted => captured = true,
            Some(b'}') if !quoted => captured = false,
            Some(_) | None => {}
        }
    }
    identifiers
}

#[test]
fn live_dual_reader_sql_has_no_local_identifiers() {
    for (name, source) in [
        ("app meetings", include_str!("../../src/db/meetings.rs")),
        ("app dictation", include_str!("../../src/db/dictation.rs")),
        ("app search", include_str!("../../src/db/search.rs")),
        ("app JSON import", include_str!("../../src/db/migrate.rs")),
        (
            "MCP queries and fixture",
            include_str!("../../souffle-mcp/src/db.rs"),
        ),
    ] {
        // The app's historical test fixtures are pinned snapshots. The MCP
        // fixture, unlike those snapshots, must use the current identifiers.
        let source = if name.starts_with("app") {
            source.split("#[cfg(test)]").next().unwrap()
        } else {
            source
        };
        for sql in sql_literals(source) {
            assert!(
                local_identifiers(sql).is_empty(),
                "{name}: local SQL identifiers {:?}; use souffle_schema::sql constants:\n{sql}",
                local_identifiers(sql),
            );
        }
    }

    let schema = include_str!("../../src/db/schema.rs");
    for name in [
        "create_schema_version",
        "create_meetings_v3",
        "create_segments",
        "create_segments_index",
        "create_dictation_entries",
        "create_text_search",
    ] {
        let start = format!("pub fn {name}() -> String {{");
        let body = schema
            .split_once(&start)
            .unwrap()
            .1
            .split_once("\n}")
            .unwrap()
            .0;
        for sql in sql_literals(body) {
            assert!(local_identifiers(sql).is_empty(), "{name}: {sql}");
        }
    }

    let migrations = include_str!("../../src/db/mod.rs");
    // Only the ALTER statements in these shipped steps are snapshots. Live
    // schema-version queries and any new migration are checked normally.
    let frozen_alters: Vec<_> = [(6, 7), (7, 8), (8, 9), (16, 17)]
        .into_iter()
        .flat_map(|(version, next)| {
            let start = format!("if current_version < {version} {{");
            let end = format!("if current_version < {next} {{");
            let body = migrations
                .split_once(&start)
                .unwrap()
                .1
                .split_once(&end)
                .unwrap()
                .0;
            sql_literals(body)
                .into_iter()
                .filter(|sql| sql.starts_with("ALTER "))
        })
        .collect();
    assert_eq!(
        frozen_alters.len(),
        5,
        "audit the historical exemptions if migrations change"
    );
    for sql in sql_literals(migrations) {
        if frozen_alters
            .iter()
            .any(|frozen| std::ptr::eq(*frozen, sql))
        {
            continue;
        }
        assert!(
            local_identifiers(sql).is_empty(),
            "schema-version query: {sql}"
        );
    }
}

#[test]
fn guard_rejects_a_column_literal_even_after_the_contract_renames_it() {
    assert_eq!(
        local_identifiers("SELECT forgotten_column FROM {MEETINGS} WHERE {ID} = ?1"),
        ["forgotten_column"],
    );
    assert!(local_identifiers("SELECT {TITLE} FROM {MEETINGS} WHERE {ID} = ?1").is_empty());
}

#[test]
fn guard_rejects_drift_in_the_real_reader_source() {
    let reader = include_str!("../../souffle-mcp/src/db.rs");
    let drifted = reader.replace(
        "m.{TITLE}",
        &format!("m.{}", souffle_schema::sql::column::TITLE),
    );
    assert_ne!(reader, drifted, "exercise the actual list_meetings query");
    assert!(
        sql_literals(&drifted)
            .into_iter()
            .any(|sql| !local_identifiers(sql).is_empty())
    );
}
