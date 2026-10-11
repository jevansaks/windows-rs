use super::*;

pub(super) fn capture(
    traversal: &Traversal<'_>,
    output: &mut BTreeMap<Origin, SharedEnumIdentity>,
) {
    let facts: BTreeMap<_, _> = traversal
        .facts
        .iter()
        .filter(|fact| matches!(fact.kind, FactKind::Enum | FactKind::Typedef))
        .map(|fact| (&fact.origin, fact))
        .collect();
    for (alias, alias_origin) in traversal.seen.values().flatten() {
        if unsafe { clang_getCursorKind(*alias) } != CXCursor_TypedefDecl
            || !global_external(*alias)
        {
            continue;
        }
        let target =
            unsafe { clang_getTypeDeclaration(clang_getTypedefDeclUnderlyingType(*alias)) };
        if unsafe { clang_getCursorKind(target) } != CXCursor_EnumDecl || !global_external(target) {
            continue;
        }
        let canonical = unsafe { clang_getCanonicalCursor(target) };
        let Some((_, enum_origin)) = traversal
            .seen
            .get(&unsafe { clang_hashCursor(target) })
            .into_iter()
            .flatten()
            .find(|(cursor, _)| unsafe {
                clang_equalCursors(*cursor, target) != 0
                    && clang_equalCursors(clang_getCanonicalCursor(*cursor), canonical) != 0
            })
        else {
            continue;
        };
        let (Some(alias_fact), Some(enum_fact)) = (facts.get(alias_origin), facts.get(enum_origin))
        else {
            continue;
        };
        let FactData::Typedef {
            target: TypeRef::Named { name, declaration },
        } = &alias_fact.data
        else {
            continue;
        };
        if alias_fact.parent.is_some()
            || enum_fact.parent.is_some()
            || alias_fact.spelling != alias_fact.expansion
            || enum_fact.spelling != enum_fact.expansion
            || enum_fact.name != *name
            || enum_fact.spelling != *declaration
            || alias_fact.origin.tu != enum_fact.origin.tu
        {
            continue;
        }
        let FactData::Enum {
            variants,
            fixed,
            scoped,
            ..
        } = &enum_fact.data
        else {
            continue;
        };
        if *fixed || *scoped || !enum_fact.definition {
            continue;
        }
        let tokens = cursor_tokens(*alias);
        let raw: Vec<_> = tokens.iter().map(|(_, token)| token.clone()).collect();
        if !literal_definition(&tokens, &alias_fact.name, variants) {
            continue;
        }
        let enum_tokens = cursor_tokens(target);
        if enum_tokens.len() + 2 > tokens.len()
            || enum_tokens != tokens[1..1 + enum_tokens.len()]
            || enum_tokens.last().is_none_or(|(_, token)| token != "}")
        {
            continue;
        }
        let enum_usr = cx_string(unsafe { clang_getCursorUSR(target) });
        let alias_usr = cx_string(unsafe { clang_getCursorUSR(*alias) });
        if enum_usr.is_empty() || alias_usr.is_empty() {
            continue;
        }
        let definition = Arc::new(SharedEnumDefinition {
            enum_usr,
            alias_usr,
            tokens: raw,
            data: enum_fact.data.clone(),
        });
        let identity = SharedEnumIdentity {
            definition,
            enumeration: enum_origin.clone(),
        };
        output.insert(enum_origin.clone(), identity.clone());
        output.insert(alias_origin.clone(), identity);
    }
}

fn global_external(cursor: CXCursor) -> bool {
    unsafe {
        clang_getCursorLinkage(cursor) == CXLinkage_External
            && clang_getCursorKind(clang_getCursorSemanticParent(cursor))
                == CXCursor_TranslationUnit
            && clang_getCursorKind(clang_getCursorLexicalParent(cursor)) == CXCursor_TranslationUnit
            && clang_Cursor_getModule(cursor).is_null()
    }
}

fn literal_definition(tokens: &[(CXTokenKind, String)], alias: &str, variants: &[Variant]) -> bool {
    let mut position = 0;
    for expected in ["typedef", "enum", "{"] {
        if tokens
            .get(position)
            .is_none_or(|(_, token)| token != expected)
        {
            return false;
        }
        position += 1;
    }
    if variants.is_empty() {
        return false;
    }
    for (index, variant) in variants.iter().enumerate() {
        for expected in [variant.name.as_str(), "="] {
            if tokens
                .get(position)
                .is_none_or(|(_, token)| token != expected)
            {
                return false;
            }
            position += 1;
        }
        let negative = tokens.get(position).is_some_and(|(_, token)| token == "-");
        position += usize::from(negative);
        let Some((kind, token)) = tokens.get(position) else {
            return false;
        };
        if *kind != CXToken_Literal || !token.bytes().all(|byte| byte.is_ascii_digit()) {
            return false;
        }
        let Ok(value) = token.parse::<i64>() else {
            return false;
        };
        if (if negative { -value } else { value }) != variant.value {
            return false;
        }
        position += 1;
        if tokens.get(position).is_some_and(|(_, token)| token == ",") {
            position += 1;
        } else if index + 1 != variants.len() {
            return false;
        }
    }
    if tokens.get(position).is_none_or(|(_, token)| token != "}")
        || tokens
            .get(position + 1)
            .is_none_or(|(_, token)| token != alias)
    {
        return false;
    }
    position += 2;
    if tokens.get(position).is_some_and(|(_, token)| token == ";") {
        position += 1;
    }
    position == tokens.len()
}
