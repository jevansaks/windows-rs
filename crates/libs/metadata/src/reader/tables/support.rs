#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TableSupport {
    Preserved,
    Regenerated,
    Unsupported,
    Irrelevant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EcmaTable {
    pub name: &'static str,
    pub support: TableSupport,
}

pub(crate) const ECMA_TABLES: [EcmaTable; 45] = [
    regenerated("Module"),
    preserved("TypeRef"),
    preserved("TypeDef"),
    unsupported("FieldPtr"),
    preserved("Field"),
    unsupported("MethodPtr"),
    preserved("MethodDef"),
    unsupported("ParamPtr"),
    preserved("Param"),
    preserved("InterfaceImpl"),
    preserved("MemberRef"),
    preserved("Constant"),
    preserved("CustomAttribute"),
    unsupported("FieldMarshal"),
    irrelevant("DeclSecurity"),
    preserved("ClassLayout"),
    preserved("FieldLayout"),
    irrelevant("StandAloneSig"),
    preserved("EventMap"),
    unsupported("EventPtr"),
    preserved("Event"),
    preserved("PropertyMap"),
    unsupported("PropertyPtr"),
    preserved("Property"),
    preserved("MethodSemantics"),
    preserved("MethodImpl"),
    preserved("ModuleRef"),
    preserved("TypeSpec"),
    preserved("ImplMap"),
    unsupported("FieldRVA"),
    unsupported("EncLog"),
    unsupported("EncMap"),
    preserved("Assembly"),
    irrelevant("AssemblyProcessor"),
    irrelevant("AssemblyOS"),
    regenerated("AssemblyRef"),
    irrelevant("AssemblyRefProcessor"),
    irrelevant("AssemblyRefOS"),
    irrelevant("File"),
    irrelevant("ExportedType"),
    irrelevant("ManifestResource"),
    preserved("NestedClass"),
    preserved("GenericParam"),
    unsupported("MethodSpec"),
    unsupported("GenericParamConstraint"),
];

const fn preserved(name: &'static str) -> EcmaTable {
    EcmaTable {
        name,
        support: TableSupport::Preserved,
    }
}

const fn regenerated(name: &'static str) -> EcmaTable {
    EcmaTable {
        name,
        support: TableSupport::Regenerated,
    }
}

const fn unsupported(name: &'static str) -> EcmaTable {
    EcmaTable {
        name,
        support: TableSupport::Unsupported,
    }
}

const fn irrelevant(name: &'static str) -> EcmaTable {
    EcmaTable {
        name,
        support: TableSupport::Irrelevant,
    }
}

pub(crate) fn ecma_table(id: usize) -> Option<EcmaTable> {
    ECMA_TABLES.get(id).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_ecma_table_is_classified() {
        assert_eq!(ECMA_TABLES.len(), 0x2d);
        assert!(ECMA_TABLES.iter().all(|table| !table.name.is_empty()));
        assert_eq!(ecma_table(0).unwrap().name, "Module");
        assert_eq!(ecma_table(0x2c).unwrap().name, "GenericParamConstraint");
        assert!(ecma_table(0x2d).is_none());
    }

    #[test]
    fn unsupported_tables_are_explicit() {
        let unsupported: Vec<_> = ECMA_TABLES
            .iter()
            .filter(|table| table.support == TableSupport::Unsupported)
            .map(|table| table.name)
            .collect();

        assert_eq!(
            unsupported,
            [
                "FieldPtr",
                "MethodPtr",
                "ParamPtr",
                "FieldMarshal",
                "EventPtr",
                "PropertyPtr",
                "FieldRVA",
                "EncLog",
                "EncMap",
                "MethodSpec",
                "GenericParamConstraint",
            ]
        );
    }
}
