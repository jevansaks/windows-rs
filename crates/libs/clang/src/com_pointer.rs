use super::*;

pub(super) struct Context<'a, 'facts> {
    pub snapshot: &'a Snapshot,
    pub declarations: &'a DeclarationIndex<'facts>,
    pub facts_by_declaration: &'a BTreeMap<(String, Location), &'facts Fact>,
    pub source_names: Option<&'a PlanningSourceNames>,
    pub excluded_types: Option<&'a BTreeSet<String>>,
    pub source_annotations: &'a BTreeMap<Origin, Rc<RouteAnnotations>>,
    pub effective_annotations: &'a BTreeMap<Origin, Rc<RouteAnnotations>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ProviderContract {
    source_annotations: Option<Rc<RouteAnnotations>>,
    effective_annotations: Option<Rc<RouteAnnotations>>,
    projection: Option<String>,
    namespace: Option<String>,
    owner: Option<(String, Option<String>, bool, bool, bool)>,
}

struct TypeExpansion<'a> {
    structural_pairs: &'a mut BTreeSet<(Origin, Origin)>,
    left_aliases: BTreeSet<Origin>,
    right_aliases: BTreeSet<Origin>,
    canonical: Option<(&'a NativeType, &'a NativeType)>,
    left_contracts: Vec<(Origin, ProviderContract)>,
    right_contracts: Vec<(Origin, ProviderContract)>,
}

impl<'a> TypeExpansion<'a> {
    fn new(structural_pairs: &'a mut BTreeSet<(Origin, Origin)>) -> Self {
        Self {
            structural_pairs,
            left_aliases: BTreeSet::new(),
            right_aliases: BTreeSet::new(),
            canonical: None,
            left_contracts: Vec::new(),
            right_contracts: Vec::new(),
        }
    }
}

struct RecordLayout<'a> {
    base: Option<&'a TypeRef>,
    fields: &'a [Field],
    size: i64,
    align: i64,
    packing: Option<i64>,
    alignment: Option<i64>,
    union: bool,
}

impl<'a> From<&'a InlineRecord> for RecordLayout<'a> {
    fn from(record: &'a InlineRecord) -> Self {
        Self {
            base: record.base.as_ref(),
            fields: &record.fields,
            size: record.size,
            align: record.align,
            packing: record.packing,
            alignment: record.alignment,
            union: record.union,
        }
    }
}

impl<'facts> Context<'_, 'facts> {
    pub(super) fn exact(&self, tu: &str, name: &str, location: &Location) -> Option<&'facts Fact> {
        let fact = *self
            .facts_by_declaration
            .get(&(tu.to_string(), location.clone()))?;
        if name != fact.name
            && self
                .source_names
                .is_none_or(|names| names.fact_name(fact) != name)
        {
            return None;
        }
        let bucket = self.declarations.get(tu, &fact.name, location);
        (!bucket.is_empty()
            && bucket
                .iter()
                .all(|other| same_source_declaration(fact, other)))
        .then_some(fact)
    }

    fn contracts_equal(
        &self,
        left: &Fact,
        right: &Fact,
        seen: &mut BTreeSet<(Origin, Origin)>,
    ) -> Result<(), String> {
        let (Some(a), Some(b)) = (
            self.snapshot.native_com_contracts.get(&left.origin),
            self.snapshot.native_com_contracts.get(&right.origin),
        ) else {
            return Err(format!(
                "missing native contract for {:?} or {:?}",
                left.origin, right.origin
            ));
        };
        if a.guid != b.guid
            || a.pointer_width != b.pointer_width
            || a.bases != b.bases
            || a.slots.len() != b.slots.len()
        {
            return Err(format!(
                "contract header: IID {:?}/{:?}, pointer width {}/{}, bases {:?}/{:?}, slots {}/{}",
                a.guid,
                b.guid,
                a.pointer_width,
                b.pointer_width,
                a.bases,
                b.bases,
                a.slots.len(),
                b.slots.len()
            ));
        }
        let pair = (left.origin.clone(), right.origin.clone());
        if !seen.insert(pair.clone()) {
            return Ok(());
        }
        let result = (|| {
            for (index, (a, b)) in a.slots.iter().zip(&b.slots).enumerate() {
                if a.name != b.name
                    || a.convention != b.convention
                    || a.params.len() != b.params.len()
                {
                    return Err(format!(
                        "slot {index}: name {:?}/{:?}, native convention {}/{}, parameters {}/{}",
                        a.name,
                        b.name,
                        a.convention,
                        b.convention,
                        a.params.len(),
                        b.params.len()
                    ));
                }
                self.types_equal(
                    &a.result,
                    &left.origin.tu,
                    &b.result,
                    &right.origin.tu,
                    false,
                    seen,
                )
                .map_err(|reason| format!("slot {index} `{}` -> result -> {reason}", a.name))?;
                for (parameter, (a_param, b_param)) in a.params.iter().zip(&b.params).enumerate() {
                    let matched = if a_param.annotation.size.is_some()
                        || b_param.annotation.size.is_some()
                    {
                        if let (Some((a_mut, a_target)), Some((b_mut, b_target))) = (
                            self.pointer_target(&a_param.ty, &left.origin.tu),
                            self.pointer_target(&b_param.ty, &right.origin.tu),
                        ) {
                            if a_mut != b_mut {
                                Err("counted pointer mutability differs".to_string())
                            } else {
                                self.types_equal(
                                    a_target,
                                    &left.origin.tu,
                                    b_target,
                                    &right.origin.tu,
                                    false,
                                    seen,
                                )
                            }
                        } else {
                            Err("counted parameter has no exact native pointer target".to_string())
                        }
                    } else {
                        self.types_equal(
                            &a_param.ty,
                            &left.origin.tu,
                            &b_param.ty,
                            &right.origin.tu,
                            false,
                            seen,
                        )
                    };
                    matched.map_err(|reason| {
                        format!(
                            "slot {index} `{}` -> parameter {parameter} -> {reason}",
                            a.name
                        )
                    })?;
                }
            }
            Ok(())
        })();
        seen.remove(&pair);
        result
    }

    fn pointer_target<'b>(&'b self, ty: &'b TypeRef, tu: &str) -> Option<(bool, &'b TypeRef)> {
        let mut current = ty;
        let mut seen = BTreeSet::new();
        loop {
            match current {
                TypeRef::Pointer { mutable, target } => return Some((*mutable, target)),
                TypeRef::Named { name, declaration } => {
                    let fact = self.exact(tu, name, declaration)?;
                    if !seen.insert(&fact.origin) {
                        return None;
                    }
                    let FactData::Typedef { target } = &fact.data else {
                        return None;
                    };
                    current = target;
                }
                _ => return None,
            }
        }
    }

    fn records_equal(
        &self,
        a: RecordLayout<'_>,
        a_tu: &str,
        b: RecordLayout<'_>,
        b_tu: &str,
        origins: Option<(&Origin, &Origin)>,
        expansion: &mut TypeExpansion<'_>,
    ) -> Result<(), String> {
        if (a.size, a.align, a.packing, a.alignment, a.union)
            != (b.size, b.align, b.packing, b.alignment, b.union)
            || a.fields.len() != b.fields.len()
        {
            return Err(format!(
                "record layout: size {}/{}, alignment {}/{}, packing {:?}/{:?}, \
                 explicit alignment {:?}/{:?}, union {}/{}, fields {}/{}",
                a.size,
                b.size,
                a.align,
                b.align,
                a.packing,
                b.packing,
                a.alignment,
                b.alignment,
                a.union,
                b.union,
                a.fields.len(),
                b.fields.len()
            ));
        }
        let pair = origins.map(|(a, b)| (a.clone(), b.clone()));
        if pair
            .as_ref()
            .is_some_and(|pair| !expansion.structural_pairs.insert(pair.clone()))
        {
            return if expansion.canonical.is_some() {
                Err("unproved canonical record cycle".into())
            } else {
                Ok(())
            };
        }
        let result = (|| {
            let native = match expansion.canonical {
                Some((left, right)) => {
                    let (NativeTypeLayer::Record(left), NativeTypeLayer::Record(right)) =
                        (&left.layer, &right.layer)
                    else {
                        return Err("missing paired native record layers".into());
                    };
                    if a.base.is_some()
                        || b.base.is_some()
                        || left.align != a.align
                        || right.align != b.align
                        || left.fields.len() != a.fields.len()
                        || right.fields.len() != b.fields.len()
                        || a.union != (left.declaration.kind == clang_sys::CXCursor_UnionDecl)
                        || b.union != (right.declaration.kind == clang_sys::CXCursor_UnionDecl)
                    {
                        return Err("native record layout/member binding differs".into());
                    }
                    Some((left.as_ref(), right.as_ref()))
                }
                None => None,
            };
            match (a.base, b.base) {
                (Some(a_base), Some(b_base)) => self
                    .types_equal_expanded(a_base, a_tu, b_base, b_tu, false, expansion)
                    .map_err(|reason| format!("record base -> {reason}"))?,
                (None, None) => {}
                _ => return Err("record base presence differs".to_string()),
            }
            let mut anonymous = 0;
            for (index, (a, b)) in a.fields.iter().zip(b.fields).enumerate() {
                if (a.offset, a.size, a.align, a.bit_width)
                    != (b.offset, b.size, b.align, b.bit_width)
                {
                    return Err(format!(
                        "field {index}: offset {}/{}, size {}/{}, alignment {}/{}, \
                         bit width {:?}/{:?}",
                        a.offset,
                        b.offset,
                        a.size,
                        b.size,
                        a.align,
                        b.align,
                        a.bit_width,
                        b.bit_width
                    ));
                }
                let previous = expansion.canonical;
                if let Some((left, right)) = native {
                    let (ln, rn) = (&left.fields[index], &right.fields[index]);
                    if ln.declaration.anonymous {
                        anonymous += 1;
                    }
                    self.canonical_field(a, a_tu, ln, left, index, anonymous)
                        .and_then(|()| self.canonical_field(b, b_tu, rn, right, index, anonymous))
                        .map_err(|reason| format!("field[{index}]({}) -> {reason}", a.name))?;
                    if ln.declaration.spelling != rn.declaration.spelling
                        || ln.declaration.expansion != rn.declaration.expansion
                        || ln.declaration.kind != rn.declaration.kind
                        || ln.declaration.name != rn.declaration.name
                        || ln.declaration.anonymous != rn.declaration.anonymous
                    {
                        return Err(format!("field[{index}] native declaration differs"));
                    }
                    expansion.canonical = Some((&ln.ty, &rn.ty));
                }
                let result = self.types_equal_expanded(&a.ty, a_tu, &b.ty, b_tu, false, expansion);
                expansion.canonical = previous;
                result.map_err(|reason| {
                    if native.is_some() {
                        format!("field[{index}]({}) -> {reason}", a.name)
                    } else {
                        format!("field {index} -> {reason}")
                    }
                })?;
            }
            Ok(())
        })();
        if let Some(pair) = pair {
            expansion.structural_pairs.remove(&pair);
        }
        result
    }

    fn canonical_field(
        &self,
        field: &Field,
        tu: &str,
        native: &NativeField,
        parent: &NativeRecord,
        ordinal: usize,
        anonymous: usize,
    ) -> Result<(), String> {
        use clang_sys::*;
        let declaration = &native.declaration;
        if declaration.tu != tu
            || native.ordinal != ordinal
            || declaration.parent.kind != parent.declaration.kind
            || declaration.parent.name != parent.declaration.name
            || declaration.parent.spelling.as_ref() != Some(&parent.declaration.spelling)
            || declaration.parent.expansion.as_ref() != Some(&parent.declaration.expansion)
            || declaration.spelling.file != parent.declaration.spelling.file
            || (field.offset, field.align, field.size, field.bit_width)
                != (
                    native.offset,
                    native.align,
                    native.ty.size,
                    native.bit_width,
                )
        {
            return Err("native field source/parent/ordinal/layout binding differs".into());
        }
        if declaration.anonymous
            && matches!(declaration.kind, CXCursor_StructDecl | CXCursor_UnionDecl)
        {
            let (TypeRef::InlineRecord(_), NativeTypeLayer::Record(record)) =
                (&field.ty, &native.ty.layer)
            else {
                return Err("anonymous native member has no inline record".into());
            };
            let expected = if anonymous == 1 {
                "Anonymous".into()
            } else {
                format!("Anonymous{anonymous}")
            };
            if &record.declaration != declaration || field.name != expected {
                return Err("anonymous native member declaration binding differs".into());
            }
        } else if declaration.kind != CXCursor_FieldDecl || declaration.name != field.name {
            return Err("native field kind/name binding differs".into());
        }
        Ok(())
    }

    fn types_equal(
        &self,
        a: &TypeRef,
        a_tu: &str,
        b: &TypeRef,
        b_tu: &str,
        pointer: bool,
        seen: &mut BTreeSet<(Origin, Origin)>,
    ) -> Result<(), String> {
        self.types_equal_expanded(a, a_tu, b, b_tu, pointer, &mut TypeExpansion::new(seen))
    }

    pub(super) fn canonical_types_equal(
        &self,
        a: &TypeRef,
        a_tu: &str,
        b: &TypeRef,
        b_tu: &str,
        native: (&NativeType, &NativeType),
    ) -> Result<(), String> {
        fn supported(ty: &NativeType) -> bool {
            use clang_sys::*;
            if ty.address_space != 0 {
                return false;
            }

            let qualifiers = format!(
                "{}{}{}",
                if ty.is_const { "const" } else { "" },
                if ty.is_volatile { "volatile" } else { "" },
                if ty.is_restrict { "__restrict" } else { "" }
            );
            let compact = ty.spelling.replace(' ', "");
            match &ty.layer {
                NativeTypeLayer::Primitive => {
                    let name = match ty.kind {
                        CXType_Void => "void",
                        CXType_Bool => "bool",
                        CXType_Char_U | CXType_Char_S => "char",
                        CXType_UChar => "unsignedchar",
                        CXType_SChar => "signedchar",
                        CXType_WChar => "wchar_t",
                        CXType_Char16 => "char16_t",
                        CXType_Char32 => "char32_t",
                        CXType_Short => "short",
                        CXType_UShort => "unsignedshort",
                        CXType_Int => "int",
                        CXType_UInt => "unsignedint",
                        CXType_Long => "long",
                        CXType_ULong => "unsignedlong",
                        CXType_LongLong => "longlong",
                        CXType_ULongLong => "unsignedlonglong",
                        CXType_Float => "float",
                        CXType_Double => "double",
                        CXType_LongDouble => "longdouble",
                        _ => return false,
                    };
                    compact == format!("{qualifiers}{name}")
                }
                NativeTypeLayer::Indirect(target) => {
                    let operator = match ty.kind {
                        CXType_Pointer if matches!(ty.size, 4 | 8) => "*",
                        CXType_LValueReference if ty.size >= 0 && ty.size == target.size => "&",
                        _ => return false,
                    };
                    supported(target)
                        && compact
                            == format!("{}{operator}{qualifiers}", target.spelling.replace(' ', ""))
                }
                NativeTypeLayer::Record(record) => {
                    ty.kind == CXType_Record
                        && ty.size >= 0
                        && record.fields.iter().all(|field| supported(&field.ty))
                }
                NativeTypeLayer::Enum(enumeration) => {
                    ty.kind == CXType_Enum
                        && ty.size >= 0
                        && enumeration.underlying.layer == NativeTypeLayer::Primitive
                        && supported(&enumeration.underlying)
                }
                // Function exception specifications are not proved.
                NativeTypeLayer::Function { .. } => false,
                NativeTypeLayer::Array { target, .. } => supported(target),
            }
        }
        if !(matches!(
            &native.0.layer,
            NativeTypeLayer::Record(_) | NativeTypeLayer::Enum(_)
        ) || matches!(
            &native.1.layer,
            NativeTypeLayer::Record(_) | NativeTypeLayer::Enum(_)
        ) || (native.0.kind == clang_sys::CXType_LValueReference
            && native.1.kind == clang_sys::CXType_LValueReference))
            && native.0 != native.1
        {
            return Err(format!(
                "canonical native evidence differs: {:?} versus {:?}",
                native.0, native.1
            ));
        }
        if !supported(native.0) || !supported(native.1) {
            return Err(format!(
                "unsupported canonical native kind/modifier/address-space evidence: {:?}",
                native.0
            ));
        }
        let mut seen = BTreeSet::new();
        let mut expansion = TypeExpansion::new(&mut seen);
        expansion.canonical = Some(native);
        self.types_equal_expanded(a, a_tu, b, b_tu, false, &mut expansion)
    }

    fn canonical_nominal_declaration(
        &self,
        tu: &str,
        native: &NativeType,
        terminal: Option<&'facts Fact>,
    ) -> Result<&'facts Fact, String> {
        use clang_sys::*;
        let declaration = match &native.layer {
            NativeTypeLayer::Record(record) if native.kind == CXType_Record => &record.declaration,
            NativeTypeLayer::Enum(enumeration) if native.kind == CXType_Enum => {
                &enumeration.declaration
            }
            _ => return Err("native nominal binding: missing eligible nominal layer".into()),
        };
        if declaration.tu != tu {
            return Err(format!(
                "native nominal binding: declaration TU {:?} does not bind observation {tu:?}",
                declaration.tu
            ));
        }
        let fact = terminal
            .or_else(|| self.exact(tu, &declaration.name, &declaration.spelling))
            .ok_or_else(|| "native nominal binding: missing exact declaration".to_string())?;
        if fact.origin.tu != tu
            || fact.spelling != declaration.spelling
            || self
                .source_names
                .map_or(fact.name.as_str(), |names| names.fact_name(fact))
                != declaration.name
        {
            return Err("native nominal binding: projected terminal/source/name differs".into());
        }
        let kind = match fact.kind {
            FactKind::Struct => CXCursor_StructDecl,
            FactKind::Union => CXCursor_UnionDecl,
            FactKind::Enum => CXCursor_EnumDecl,
            _ => return Err("native nominal binding: unsupported declaration kind".into()),
        };
        if !fact.definition
            || kind != declaration.kind
            || fact.expansion != declaration.expansion
            || self.declarations.get(tu, &fact.name, &fact.spelling).len() != 1
        {
            return Err(
                "native nominal binding: incomplete, conflicting or ambiguous declaration".into(),
            );
        }
        match declaration.parent.kind {
            CXCursor_TranslationUnit
                if fact.parent.is_none()
                    && declaration.parent.name == declaration.tu
                    && declaration.parent.spelling.is_none()
                    && declaration.parent.expansion.is_none() => {}
            CXCursor_Namespace => {
                let location = declaration.parent.spelling.as_ref().ok_or_else(|| {
                    "native nominal binding: missing namespace parent source".to_string()
                })?;
                let parent = self
                    .exact(tu, &declaration.parent.name, location)
                    .ok_or_else(|| {
                        "native nominal binding: unresolved exact namespace parent".to_string()
                    })?;
                if parent.kind != FactKind::Namespace
                    || fact.parent.as_ref() != Some(&parent.origin)
                    || declaration.parent.expansion.as_ref() != Some(&parent.expansion)
                    || self.declarations.get(tu, &parent.name, location).len() != 1
                {
                    return Err("native nominal binding: namespace parent differs".into());
                }
            }
            _ => return Err("native nominal binding: unsupported semantic parent".into()),
        }
        Ok(fact)
    }

    fn canonical_nominal_context(
        &self,
        a: &Fact,
        b: &Fact,
        native: (&NativeType, &NativeType),
    ) -> Result<(), String> {
        self.canonical_nominal_declaration(&a.origin.tu, native.0, Some(a))?;
        self.canonical_nominal_declaration(&b.origin.tu, native.1, Some(b))?;
        let (left, right) = match (&native.0.layer, &native.1.layer) {
            (NativeTypeLayer::Record(left), NativeTypeLayer::Record(right)) => {
                (&left.declaration, &right.declaration)
            }
            (NativeTypeLayer::Enum(left), NativeTypeLayer::Enum(right)) => {
                (&left.declaration, &right.declaration)
            }
            _ => return Err("native nominal binding: mismatched declaration layers".into()),
        };
        if a.spelling != b.spelling
            || a.kind != b.kind
            || left.parent.kind != right.parent.kind
            || (left.parent.kind != clang_sys::CXCursor_TranslationUnit
                && left.parent != right.parent)
        {
            return Err("native nominal binding: physical declaration/parent differs".into());
        }
        if self.provider_contract(a)? != self.provider_contract(b)? {
            let kind = if a.kind == FactKind::Enum {
                "enum"
            } else {
                "record"
            };
            return Err(format!("canonical {kind} provider contract differs"));
        }
        Ok(())
    }

    fn canonical_enums_equal(
        &self,
        a: &Fact,
        b: &Fact,
        native: (&NativeType, &NativeType),
    ) -> Result<(), String> {
        use clang_sys::*;
        let (NativeTypeLayer::Enum(left), NativeTypeLayer::Enum(right)) =
            (&native.0.layer, &native.1.layer)
        else {
            return Err("missing paired native enum layers".into());
        };
        let metadata = |fact: &Fact, enumeration: &NativeEnum| -> Result<(), String> {
            let FactData::Enum {
                repr,
                variants,
                scoped,
                ..
            } = &fact.data
            else {
                return Err("enum projected terminal is not an enum".into());
            };
            let repr_matches = matches!(
                (enumeration.underlying.kind, repr),
                (CXType_Char_S | CXType_SChar, Scalar::I8)
                    | (CXType_Char_U | CXType_UChar, Scalar::U8)
                    | (CXType_Short, Scalar::I16)
                    | (CXType_UShort | CXType_WChar | CXType_Char16, Scalar::U16)
                    | (CXType_Int | CXType_Long, Scalar::I32)
                    | (CXType_UInt | CXType_ULong | CXType_Char32, Scalar::U32)
                    | (CXType_LongLong, Scalar::I64)
                    | (CXType_ULongLong, Scalar::U64)
            );
            if !repr_matches
                || *scoped != enumeration.scoped
                || variants.len() != enumeration.variants.len()
            {
                return Err("enum underlying/scoped/variant count metadata binding differs".into());
            }
            for (ordinal, (projected, native)) in
                variants.iter().zip(&enumeration.variants).enumerate()
            {
                let declaration = &native.declaration;
                if declaration.tu != fact.origin.tu
                    || declaration.kind != CXCursor_EnumConstantDecl
                    || declaration.name != projected.name
                    || declaration.name.is_empty()
                    || declaration.anonymous
                    || declaration.spelling.file != fact.spelling.file
                    || declaration.expansion.file != fact.expansion.file
                    || declaration.parent.kind != enumeration.declaration.kind
                    || declaration.parent.name != enumeration.declaration.name
                    || declaration.parent.spelling.as_ref()
                        != Some(&enumeration.declaration.spelling)
                    || declaration.parent.expansion.as_ref()
                        != Some(&enumeration.declaration.expansion)
                    || !equivalent_enum_value(projected.value, native.signed_value, *repr)
                    || native.unsigned_value != native.signed_value as u64
                {
                    return Err(format!(
                        "enum variant[{ordinal}] declaration/value metadata binding differs"
                    ));
                }
            }
            Ok(())
        };
        metadata(a, left)?;
        metadata(b, right)?;
        if left.underlying != right.underlying {
            return Err(format!(
                "enum underlying native evidence differs: {:?}/{:?}",
                left.underlying, right.underlying
            ));
        }
        if left.scoped != right.scoped || left.variants.len() != right.variants.len() {
            return Err("enum scoped/ordered variant count differs".into());
        }
        for (ordinal, (left, right)) in left.variants.iter().zip(&right.variants).enumerate() {
            if left.declaration.name != right.declaration.name
                || left.declaration.kind != right.declaration.kind
                || left.declaration.spelling != right.declaration.spelling
                || left.declaration.expansion != right.declaration.expansion
                || left.signed_value != right.signed_value
                || left.unsigned_value != right.unsigned_value
            {
                return Err(format!("enum variant[{ordinal}] native evidence differs"));
            }
        }
        if a.data != b.data {
            return Err("enum projected metadata differs".into());
        }
        Ok(())
    }

    pub(super) fn interface_owners_equal(&self, a: &Fact, b: &Fact) -> Result<(), String> {
        let contribution = |fact: &Fact| {
            let name = self
                .source_names
                .map_or(fact.name.as_str(), |names| names.fact_name(fact));
            let owner = self.snapshot.root_owners.get(&fact.origin).or_else(|| {
                self.snapshot
                    .root_partitions
                    .get(&(fact.origin.tu.clone(), fact.spelling.file.clone()))
            });
            let input = self
                .snapshot
                .partition_inputs
                .get(&fact.origin.tu)
                .map_or(fact.origin.tu.as_str(), String::as_str);
            if owner.is_some_and(|owner| {
                owner.input != input
                    || owner.root != fact.spelling.file
                    || owner.exclusions.contains(name)
            }) {
                return Err(format!(
                    "interface source ownership or exclusion differs at {:?}: {owner:?}",
                    fact.origin
                ));
            }
            Ok(owner.map(|owner| {
                (
                    self.snapshot
                        .fact_namespace_authorities
                        .get(&fact.origin)
                        .unwrap_or(&owner.namespace)
                        .clone(),
                    owner.remaps.get(name).cloned(),
                    owner.u32_types.contains(name),
                    owner.flags.contains(name),
                    owner.preserved_auto_function_pointer_levels.contains(name),
                )
            }))
        };
        match (contribution(a)?, contribution(b)?) {
            (Some(left), Some(right)) if left != right => Err(format!(
                "interface source owner contributions differ: {left:?}/{right:?}"
            )),
            (Some(owned), None) | (None, Some(owned))
                if owned.1.is_some() || owned.2 || owned.3 || owned.4 =>
            {
                Err("unsupported one-sided interface owner API contribution".into())
            }
            // An observation with no owner contributes no routing policy; existing root routing
            // selects the namespace. This is not missing-owner equality or authority selection.
            _ => Ok(()),
        }
    }

    fn canonical_provider(
        &self,
        fact: &Fact,
        native: &NativeType,
    ) -> Result<ProviderContract, String> {
        if self
            .snapshot
            .native_typedef_types
            .get(&fact.origin)
            .and_then(Option::as_ref)
            != Some(native)
            || self.snapshot.suppressed_type_origins.contains(&fact.origin)
            || self
                .excluded_types
                .is_some_and(|excluded| excluded.contains(&fact.name))
            || fact.parent.is_some()
            || self
                .declarations
                .get(&fact.origin.tu, &fact.name, &fact.spelling)
                .len()
                != 1
        {
            return Err(format!(
                "native nominal binding/provider: ineligible canonical typedef provider {:?}",
                fact.origin
            ));
        }
        self.provider_contract(fact)
    }

    fn provider_contract(&self, fact: &Fact) -> Result<ProviderContract, String> {
        let source_name = self
            .source_names
            .map_or(fact.name.as_str(), |names| names.fact_name(fact));
        if self.snapshot.suppressed_type_origins.contains(&fact.origin)
            || self
                .excluded_types
                .is_some_and(|excluded| excluded.contains(&fact.name))
        {
            return Err(format!(
                "suppressed or excluded canonical provider {:?}",
                fact.origin
            ));
        }
        let owner = self.snapshot.root_owners.get(&fact.origin).or_else(|| {
            self.snapshot
                .root_partitions
                .get(&(fact.origin.tu.clone(), fact.spelling.file.clone()))
        });
        let input = self
            .snapshot
            .partition_inputs
            .get(&fact.origin.tu)
            .map_or(fact.origin.tu.as_str(), String::as_str);
        if owner.is_some_and(|owner| {
            owner.input != input
                || owner.root != fact.spelling.file
                || owner.exclusions.contains(source_name)
        }) {
            return Err(format!(
                "unsupported source ownership or excluded provider {:?}: authored name \
                 `{source_name}`, input `{input}`, source {:?}, owner {:?}",
                fact.origin, fact.spelling, owner
            ));
        }
        Ok(ProviderContract {
            source_annotations: self.source_annotations.get(&fact.origin).cloned(),
            effective_annotations: self.effective_annotations.get(&fact.origin).cloned(),
            projection: self
                .snapshot
                .projected_type_names
                .get(&fact.origin)
                .cloned(),
            namespace: self
                .snapshot
                .fact_namespace_authorities
                .get(&fact.origin)
                .cloned(),
            owner: owner.map(|owner| {
                (
                    owner.namespace.clone(),
                    owner.remaps.get(source_name).cloned(),
                    owner.u32_types.contains(source_name),
                    owner.flags.contains(source_name),
                    owner
                        .preserved_auto_function_pointer_levels
                        .contains(source_name),
                )
            }),
        })
    }

    fn types_equal_expanded(
        &self,
        a: &TypeRef,
        a_tu: &str,
        b: &TypeRef,
        b_tu: &str,
        pointer: bool,
        expansion: &mut TypeExpansion<'_>,
    ) -> Result<(), String> {
        if let TypeRef::Named { name, declaration } = a {
            let Some(fact) = self.exact(a_tu, name, declaration) else {
                return Err(format!(
                    "native nominal binding: unresolved exact declaration `{name}` at {}:{} in `{a_tu}`",
                    declaration.file, declaration.offset
                ));
            };
            if let FactData::Typedef { target } = &fact.data {
                if let Some((native, _)) = expansion.canonical {
                    expansion
                        .left_contracts
                        .push((fact.origin.clone(), self.canonical_provider(fact, native)?));
                }
                if !expansion.left_aliases.insert(fact.origin.clone()) {
                    return Err(format!("typedef cycle at {:?}", fact.origin));
                }
                let result = self.types_equal_expanded(target, a_tu, b, b_tu, pointer, expansion);
                expansion.left_aliases.remove(&fact.origin);
                if expansion.canonical.is_some() {
                    expansion.left_contracts.pop();
                }
                return result
                    .map_err(|reason| format!("alias `{name}` {:?} -> {reason}", fact.origin));
            }
        }
        if let TypeRef::Named { name, declaration } = b {
            let Some(fact) = self.exact(b_tu, name, declaration) else {
                return Err(format!(
                    "native nominal binding: unresolved exact declaration `{name}` at {}:{} in `{b_tu}`",
                    declaration.file, declaration.offset
                ));
            };
            if let FactData::Typedef { target } = &fact.data {
                if let Some((_, native)) = expansion.canonical {
                    expansion
                        .right_contracts
                        .push((fact.origin.clone(), self.canonical_provider(fact, native)?));
                }
                if !expansion.right_aliases.insert(fact.origin.clone()) {
                    return Err(format!("typedef cycle at {:?}", fact.origin));
                }
                let result = self.types_equal_expanded(a, a_tu, target, b_tu, pointer, expansion);
                expansion.right_aliases.remove(&fact.origin);
                if expansion.canonical.is_some() {
                    expansion.right_contracts.pop();
                }
                return result
                    .map_err(|reason| format!("alias `{name}` {:?} -> {reason}", fact.origin));
            }
        }
        if expansion.canonical.is_some()
            && !expansion
                .left_contracts
                .iter()
                .map(|(_, contract)| contract)
                .eq(expansion
                    .right_contracts
                    .iter()
                    .map(|(_, contract)| contract))
        {
            return Err(format!(
                "canonical provider contract differs: {:?} versus {:?}",
                expansion.left_contracts, expansion.right_contracts
            ));
        }
        if let Some((left, right)) = expansion.canonical
            && (
                left.kind,
                left.is_const,
                left.is_volatile,
                left.is_restrict,
                left.size,
                left.address_space,
                &left.spelling,
            ) != (
                right.kind,
                right.is_const,
                right.is_volatile,
                right.is_restrict,
                right.size,
                right.address_space,
                &right.spelling,
            )
        {
            return Err(format!(
                "canonical native evidence differs: kind {}/{} qualifiers {:?}/{:?} size {}/{} \
                 address-space {}/{} spelling {:?}/{:?}",
                left.kind,
                right.kind,
                (left.is_const, left.is_volatile, left.is_restrict),
                (right.is_const, right.is_volatile, right.is_restrict),
                left.size,
                right.size,
                left.address_space,
                right.address_space,
                left.spelling,
                right.spelling
            ));
        }
        match (a, b) {
            (
                TypeRef::Pointer {
                    mutable: a_mut,
                    target: a,
                },
                TypeRef::Pointer {
                    mutable: b_mut,
                    target: b,
                },
            ) if a_mut == b_mut => {
                let previous = expansion.canonical;
                if let Some((a, b)) = previous {
                    let (NativeTypeLayer::Indirect(a), NativeTypeLayer::Indirect(b)) =
                        (&a.layer, &b.layer)
                    else {
                        return Err("missing canonical pointer layer".to_string());
                    };
                    expansion.canonical = Some((a, b));
                }
                let result = self.types_equal_expanded(a, a_tu, b, b_tu, true, expansion);
                expansion.canonical = previous;
                result.map_err(|reason| format!("pointer target -> {reason}"))
            }
            (
                TypeRef::Reference {
                    mutable: a_mut,
                    target: a,
                },
                TypeRef::Reference {
                    mutable: b_mut,
                    target: b,
                },
            ) if a_mut == b_mut => {
                let previous = expansion.canonical;
                if let Some((left, right)) = previous {
                    if left.kind != clang_sys::CXType_LValueReference
                        || right.kind != clang_sys::CXType_LValueReference
                    {
                        return Err("missing canonical lvalue reference kind".into());
                    }
                    let (NativeTypeLayer::Indirect(left), NativeTypeLayer::Indirect(right)) =
                        (&left.layer, &right.layer)
                    else {
                        return Err("missing canonical lvalue reference layer".into());
                    };
                    expansion.canonical = Some((left, right));
                }
                let result = self.types_equal_expanded(a, a_tu, b, b_tu, false, expansion);
                expansion.canonical = previous;
                result.map_err(|reason| format!("reference layout -> {reason}"))
            }
            (
                TypeRef::Array {
                    target: a,
                    len: a_len,
                },
                TypeRef::Array {
                    target: b,
                    len: b_len,
                },
            ) if a_len == b_len => {
                let previous = expansion.canonical;
                if let Some((a, b)) = previous {
                    let (
                        NativeTypeLayer::Array { target: a, len: an },
                        NativeTypeLayer::Array { target: b, len: bn },
                    ) = (&a.layer, &b.layer)
                    else {
                        return Err("missing canonical array layer".to_string());
                    };
                    if *an != *a_len as u64 || *bn != *b_len as u64 {
                        return Err("canonical array length binding differs".into());
                    }
                    expansion.canonical = Some((a, b));
                }
                let result = self.types_equal_expanded(a, a_tu, b, b_tu, false, expansion);
                expansion.canonical = previous;
                result.map_err(|reason| format!("array layout -> {reason}"))
            }
            (TypeRef::InlineRecord(a), TypeRef::InlineRecord(b)) => self
                .records_equal(
                    a.as_ref().into(),
                    a_tu,
                    b.as_ref().into(),
                    b_tu,
                    None,
                    expansion,
                )
                .map_err(|reason| format!("inline record -> {reason}")),
            (
                TypeRef::Named {
                    name: a_name,
                    declaration: a_location,
                },
                TypeRef::Named {
                    name: b_name,
                    declaration: b_location,
                },
            ) => {
                let (Some(a), Some(b)) = (
                    self.exact(a_tu, a_name, a_location),
                    self.exact(b_tu, b_name, b_location),
                ) else {
                    return Err(format!(
                        "unresolved exact declarations `{a_name}` at {}:{} in `{a_tu}` or \
                         `{b_name}` at {}:{} in `{b_tu}`",
                        a_location.file, a_location.offset, b_location.file, b_location.offset
                    ));
                };
                if let Some(native) = expansion.canonical {
                    self.canonical_nominal_context(a, b, native)?;
                }
                if pointer
                    && self
                        .snapshot
                        .native_com_contracts
                        .get(&a.origin)
                        .is_some_and(|contract| !contract.guid.is_empty())
                    && self
                        .snapshot
                        .native_com_contracts
                        .get(&b.origin)
                        .is_some_and(|contract| !contract.guid.is_empty())
                {
                    return self
                        .contracts_equal(a, b, expansion.structural_pairs)
                        .map_err(|reason| {
                            format!("COM pointees {:?}/{:?} -> {reason}", a.origin, b.origin)
                        });
                }
                match (&a.data, &b.data) {
                    (
                        FactData::Record {
                            base: a_base,
                            fields: a_fields,
                            size: a_size,
                            align: a_align,
                            packing: a_pack,
                            alignment: a_alignment,
                            union: a_union,
                        },
                        FactData::Record {
                            base: b_base,
                            fields: b_fields,
                            size: b_size,
                            align: b_align,
                            packing: b_pack,
                            alignment: b_alignment,
                            union: b_union,
                        },
                    ) => {
                        let left = RecordLayout {
                            base: a_base.as_ref(),
                            fields: a_fields,
                            size: *a_size,
                            align: *a_align,
                            packing: *a_pack,
                            alignment: *a_alignment,
                            union: *a_union,
                        };
                        let right = RecordLayout {
                            base: b_base.as_ref(),
                            fields: b_fields,
                            size: *b_size,
                            align: *b_align,
                            packing: *b_pack,
                            alignment: *b_alignment,
                            union: *b_union,
                        };
                        let result = if expansion.canonical.is_some() {
                            self.records_equal(
                                left,
                                a_tu,
                                right,
                                b_tu,
                                Some((&a.origin, &b.origin)),
                                expansion,
                            )
                        } else {
                            self.records_equal(
                                left,
                                a_tu,
                                right,
                                b_tu,
                                Some((&a.origin, &b.origin)),
                                &mut TypeExpansion::new(expansion.structural_pairs),
                            )
                        };
                        result.map_err(|reason| {
                            format!("record {:?}/{:?} -> {reason}", a.origin, b.origin)
                        })
                    }
                    (FactData::Enum { .. }, FactData::Enum { .. }) => {
                        if let Some(native) = expansion.canonical {
                            self.canonical_enums_equal(a, b, native)
                        } else if a.data == b.data {
                            Ok(())
                        } else {
                            Err("enum metadata differs".into())
                        }
                    }
                    (FactData::Interface { .. }, FactData::Interface { .. }) => {
                        self.contracts_equal(a, b, expansion.structural_pairs)
                    }
                    _ => Err(format!(
                        "native declaration kinds at {:?}/{:?}: {}/{}",
                        a.origin,
                        b.origin,
                        fact_data_kind(&a.data),
                        fact_data_kind(&b.data)
                    )),
                }
            }
            (TypeRef::Void, TypeRef::Void)
            | (TypeRef::String, TypeRef::String)
            | (TypeRef::Object, TypeRef::Object) => Ok(()),
            (TypeRef::Scalar(a), TypeRef::Scalar(b)) if a == b => {
                if expansion.canonical.is_some_and(|(left, right)| {
                    left.layer != NativeTypeLayer::Primitive
                        || right.layer != NativeTypeLayer::Primitive
                }) {
                    return Err("projected scalar has nonprimitive native evidence".into());
                }
                Ok(())
            }
            (
                TypeRef::OpaquePointer {
                    mutable: a_mut,
                    tag: a,
                },
                TypeRef::OpaquePointer {
                    mutable: b_mut,
                    tag: b,
                },
            ) if a_mut == b_mut && a == b => Ok(()),
            _ => Err(format!("unsupported or differing native types {a:?}/{b:?}")),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) enum Failure {
    Missing,
    Conflicting(Arc<str>),
    Ambiguous,
    Unproved,
}

impl Failure {
    pub fn message(&self, name: &str) -> String {
        let reason = match self {
            Self::Missing => "has no eligible owned interface",
            Self::Conflicting(reason) => {
                return format!(
                    "native COM record `{name}` has conflicting native COM pointer contracts: \
                     {reason}"
                );
            }
            Self::Ambiguous => "has ambiguous eligible COM interface providers",
            Self::Unproved => "has no proved native COM pointer contract",
        };
        format!("native COM record `{name}` {reason}")
    }
}

pub(super) struct Bindings<'facts> {
    providers: BTreeMap<Origin, Result<&'facts Fact, Failure>>,
}

impl<'facts> Bindings<'facts> {
    pub fn new(context: &Context<'_, 'facts>, facts: &HashMap<&Origin, &'facts Fact>) -> Self {
        let snapshot = context.snapshot;
        if !snapshot.header_partition_policy {
            return Self {
                providers: BTreeMap::new(),
            };
        }
        let mut eligible: BTreeMap<&str, Vec<&Fact>> = BTreeMap::new();
        for (origin, contract) in &snapshot.native_com_contracts {
            let Some(fact) = facts.get(origin) else {
                continue;
            };
            if !contract.guid.is_empty()
                && fact.definition
                && fact.root
                && fact.parent.is_none()
                && matches!(fact.data, FactData::Interface { .. })
                && snapshot
                    .root_owners
                    .get(origin)
                    .is_some_and(|owner| !owner.exclusions.contains(&fact.name))
                && !snapshot.suppressed_type_origins.contains(origin)
                && !context
                    .excluded_types
                    .is_some_and(|excluded| excluded.contains(&fact.name))
            {
                eligible.entry(&contract.guid).or_default().push(fact);
            }
        }
        let mut providers = BTreeMap::new();
        for origin in &snapshot.unproved_com_records {
            if facts.get(origin).is_some_and(|fact| {
                fact.parent.is_none()
                    && fact.kind == FactKind::Struct
                    && matches!(
                        fact.data,
                        FactData::Record { .. } | FactData::Unsupported { .. }
                    )
            }) && !snapshot.root_owners.contains_key(origin)
            {
                providers.insert(origin.clone(), Err(Failure::Unproved));
            }
        }
        for (origin, contract) in &snapshot.native_com_contracts {
            let Some(fact) = facts.get(origin) else {
                continue;
            };
            if contract.guid.is_empty()
                || !contract.has_fields
                || fact.parent.is_some()
                || !matches!(fact.data, FactData::Record { .. })
                || snapshot.root_owners.contains_key(origin)
            {
                continue;
            }
            let candidates = eligible
                .get(contract.guid.as_str())
                .map_or(&[][..], Vec::as_slice);
            let result = match candidates {
                [] => Err(Failure::Missing),
                [first, ..] => {
                    let owner = &snapshot.root_owners[&first.origin];
                    if candidates.iter().any(|candidate| {
                        candidate.spelling != first.spelling
                            || candidate.name != first.name
                            || !same_owner_policy(&snapshot.root_owners[&candidate.origin], owner)
                    }) {
                        Err(Failure::Ambiguous)
                    } else if let Some((candidate, reason)) =
                        candidates.iter().find_map(|candidate| {
                            context
                                .contracts_equal(fact, candidate, &mut BTreeSet::new())
                                .err()
                                .map(|reason| (*candidate, reason))
                        })
                    {
                        Err(Failure::Conflicting(format!(
                            "candidate {:?} at {}:{} owner {:?}; provider {:?} at {}:{} owner {:?}; \
                             first mismatch: {reason}",
                            fact.origin, fact.spelling.file, fact.spelling.offset,
                            snapshot.root_owners.get(&fact.origin),
                            candidate.origin, candidate.spelling.file, candidate.spelling.offset,
                            snapshot.root_owners.get(&candidate.origin)
                        ).into()))
                    } else {
                        Ok(preferred_fact(candidates))
                    }
                }
            };
            providers.insert(origin.clone(), result);
        }
        Self { providers }
    }

    pub fn provider_for(
        &self,
        fact: &Fact,
        index: &HashMap<&str, Vec<&'facts Fact>>,
    ) -> Option<Result<&'facts Fact, Failure>> {
        let target = typedef_tag_declaration(fact, index).unwrap_or(fact);
        self.providers.get(&target.origin).cloned()
    }

    pub fn validate_use(
        &self,
        ty: &TypeRef,
        tu: &str,
        annotations: &[Annotation],
        counted: bool,
        pointer: bool,
        context: &Context<'_, 'facts>,
    ) -> Result<(), Error> {
        if self.providers.is_empty() {
            return Ok(());
        }
        let counted = counted
            || annotations.iter().any(|annotation| {
                matches!(
                    annotation,
                    Annotation::ArrayCountParam(_)
                        | Annotation::ArrayCountConst(_)
                        | Annotation::ArrayCountField(_)
                        | Annotation::MemorySizeParam(_)
                        | Annotation::MemorySizeConst(_)
                )
            });
        self.validate(ty, tu, counted, pointer, context, &mut BTreeSet::new())
    }

    fn validate(
        &self,
        ty: &TypeRef,
        tu: &str,
        counted: bool,
        pointer: bool,
        context: &Context<'_, 'facts>,
        seen: &mut BTreeSet<Origin>,
    ) -> Result<(), Error> {
        match ty {
            TypeRef::Pointer { target, .. } => {
                self.validate(target, tu, false, !counted, context, seen)
            }
            TypeRef::Reference { target, .. } | TypeRef::Array { target, .. } => {
                self.validate(target, tu, false, false, context, seen)
            }
            TypeRef::Named { name, declaration } => {
                let Some(fact) = context.exact(tu, name, declaration) else {
                    if context
                        .declarations
                        .get(tu, name, declaration)
                        .iter()
                        .any(|fact| self.providers.contains_key(&fact.origin))
                    {
                        return Err(Error(format!(
                            "ambiguous exact native COM declaration `{name}` at {}:{} in \
                             translation unit `{tu}`",
                            declaration.file, declaration.offset,
                        )));
                    }
                    return Ok(());
                };
                if let Some(provider) = self.providers.get(&fact.origin) {
                    if !pointer {
                        return Err(Error(format!(
                            "native COM record `{name}` requires object layout at {}:{} in \
                             translation unit `{tu}`; only COM pointer uses can bind an interface",
                            declaration.file, declaration.offset,
                        )));
                    }
                    provider
                        .as_ref()
                        .map_err(|failure| Error(failure.message(name)))?;
                }
                if !seen.insert(fact.origin.clone()) {
                    return Ok(());
                }
                let result = match &fact.data {
                    FactData::Typedef { target } => {
                        self.validate(target, tu, counted, pointer, context, seen)
                    }
                    FactData::Record { base, fields, .. }
                        if !self.providers.contains_key(&fact.origin) =>
                    {
                        if let Some(base) = base {
                            self.validate(base, tu, false, false, context, seen)?;
                        }
                        for field in fields {
                            self.validate(&field.ty, tu, false, false, context, seen)?;
                        }
                        Ok(())
                    }
                    _ => Ok(()),
                };
                seen.remove(&fact.origin);
                result
            }
            TypeRef::FunctionPointer { params, result, .. } => {
                self.validate(result, tu, false, false, context, seen)?;
                for param in params {
                    self.validate(param, tu, false, false, context, seen)?;
                }
                Ok(())
            }
            TypeRef::InlineRecord(record) => {
                for field in &record.fields {
                    self.validate(&field.ty, tu, false, false, context, seen)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}
