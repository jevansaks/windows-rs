use super::*;
mod into_stream;

mod rec;

mod blobs;
use blobs::*;

mod strings;
use strings::*;

mod helpers;
use helpers::*;

/// Represents an ECMA-335 file in memory so that it can be built incrementally.
#[derive(Default)]
pub struct File {
    strings: Strings,
    blobs: Blobs,
    records: rec::Records,
    reference: Option<reader::Index>,

    local_types: HashSet<(String, String)>,
    TypeRef: HashMap<String, HashMap<String, TypeRef>>,
    core_type_refs: HashSet<TypeRef>,
    TypeSpec: HashMap<BlobId, TypeSpec>,
    AssemblyRef: HashMap<AssemblyRefIdentity, AssemblyRef>,
    reference_assemblies: HashMap<(String, String), AssemblyRefIdentity>,
    ModuleRef: HashMap<String, ModuleRef>,
    MemberRef: HashMap<rec::MemberRef, MemberRef>,

    // Sorted staging keeps deferred tables reproducible.
    Constant: BTreeMap<HasConstant, rec::Constant>,
    Attribute: BTreeMap<HasAttribute, Vec<rec::Attribute>>,
    GenericParam: BTreeMap<TypeOrMethodDef, Vec<rec::GenericParam>>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, Ord, PartialOrd)]
pub(crate) struct AssemblyRefIdentity {
    pub(crate) major_version: u16,
    pub(crate) minor_version: u16,
    pub(crate) build_number: u16,
    pub(crate) revision_number: u16,
    pub(crate) flags: u32,
    pub(crate) public_key_or_token: Vec<u8>,
    pub(crate) name: String,
    pub(crate) culture: String,
    pub(crate) hash_value: Vec<u8>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, Ord, PartialOrd)]
pub(crate) struct AssemblyIdentity {
    pub(crate) major_version: u16,
    pub(crate) minor_version: u16,
    pub(crate) build_number: u16,
    pub(crate) revision_number: u16,
    pub(crate) flags: u32,
    pub(crate) public_key: Vec<u8>,
    pub(crate) name: String,
    pub(crate) culture: String,
}

impl AssemblyRefIdentity {
    fn named(name: &str) -> Self {
        if name == "System" {
            Self::system()
        } else {
            Self::external(name)
        }
    }

    fn system() -> Self {
        Self {
            major_version: 4,
            minor_version: 0,
            build_number: 0,
            revision_number: 0,
            flags: 0,
            public_key_or_token: vec![0xB7, 0x7A, 0x5C, 0x56, 0x19, 0x34, 0xE0, 0x89],
            name: "mscorlib".to_string(),
            culture: String::new(),
            hash_value: vec![],
        }
    }

    fn external(name: &str) -> Self {
        Self {
            major_version: 0xFF,
            minor_version: 0xFF,
            build_number: 0xFF,
            revision_number: 0xFF,
            flags: AssemblyFlags::WindowsRuntime.0,
            public_key_or_token: vec![],
            name: name.to_string(),
            culture: String::new(),
            hash_value: vec![],
        }
    }

    pub(crate) fn matches_definition(&self, definition: &AssemblyIdentity) -> bool {
        if !self.name.eq_ignore_ascii_case(&definition.name)
            || !self.culture.eq_ignore_ascii_case(&definition.culture)
        {
            return false;
        }

        let reference_is_winrt = self.flags & AssemblyFlags::WindowsRuntime.0 != 0;
        let definition_is_winrt = definition.flags & AssemblyFlags::WindowsRuntime.0 != 0;
        if reference_is_winrt != definition_is_winrt {
            return false;
        }

        let reference_version = (
            self.major_version,
            self.minor_version,
            self.build_number,
            self.revision_number,
        );
        let definition_version = (
            definition.major_version,
            definition.minor_version,
            definition.build_number,
            definition.revision_number,
        );
        const WINRT_VERSION_WILDCARD: (u16, u16, u16, u16) = (0xFF, 0xFF, 0xFF, 0xFF);
        if reference_version != definition_version
            && (!reference_is_winrt
                || (reference_version != WINRT_VERSION_WILDCARD
                    && definition_version != WINRT_VERSION_WILDCARD))
        {
            return false;
        }

        if self.flags & AssemblyFlags::PublicKey.0 != 0 {
            self.public_key_or_token == definition.public_key
        } else {
            self.public_key_or_token == public_key_token(&definition.public_key)
        }
    }

    fn from_definition(value: reader::Assembly<'_>) -> Self {
        if value.name() == "System" {
            return Self::system();
        }

        let (major_version, minor_version, build_number, revision_number) = value.version();
        Self {
            major_version,
            minor_version,
            build_number,
            revision_number,
            flags: value.flags().0,
            public_key_or_token: value.public_key().to_vec(),
            name: value.name().to_string(),
            culture: value.culture().to_string(),
            hash_value: vec![],
        }
    }
}

impl From<reader::Assembly<'_>> for AssemblyIdentity {
    fn from(value: reader::Assembly<'_>) -> Self {
        let (major_version, minor_version, build_number, revision_number) = value.version();
        Self {
            major_version,
            minor_version,
            build_number,
            revision_number,
            flags: value.flags().0,
            public_key: value.public_key().to_vec(),
            name: value.name().to_string(),
            culture: value.culture().to_string(),
        }
    }
}

impl From<reader::AssemblyRef<'_>> for AssemblyRefIdentity {
    fn from(value: reader::AssemblyRef<'_>) -> Self {
        let (major_version, minor_version, build_number, revision_number) = value.version();
        Self {
            major_version,
            minor_version,
            build_number,
            revision_number,
            flags: value.flags().0,
            public_key_or_token: value.public_key_or_token().to_vec(),
            name: value.name().to_string(),
            culture: value.culture().to_string(),
            hash_value: value.hash_value().to_vec(),
        }
    }
}

fn public_key_token(public_key: &[u8]) -> Vec<u8> {
    if public_key.is_empty() {
        return vec![];
    }
    let hash = sha1(public_key);
    hash[12..].iter().rev().copied().collect()
}

fn sha1(input: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476, 0xc3d2e1f0];
    let bit_len = (input.len() as u64) * 8;
    let padded_len = (input.len() + 1 + 8 + 63) & !63;
    let mut message = vec![0u8; padded_len];
    message[..input.len()].copy_from_slice(input);
    message[input.len()] = 0x80;
    message[padded_len - 8..].copy_from_slice(&bit_len.to_be_bytes());

    for chunk in message.chunks(64) {
        let mut words = [0u32; 80];
        for (index, word) in words[..16].iter_mut().enumerate() {
            *word = u32::from_be_bytes(chunk[index * 4..index * 4 + 4].try_into().unwrap());
        }
        for index in 16..80 {
            words[index] =
                (words[index - 3] ^ words[index - 8] ^ words[index - 14] ^ words[index - 16])
                    .rotate_left(1);
        }

        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (index, &word) in words.iter().enumerate() {
            let (f, k): (u32, u32) = match index {
                0..=19 => ((b & c) | (!b & d), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let next = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = next;
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }

    let mut result = [0u8; 20];
    for (index, &word) in h.iter().enumerate() {
        result[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    result
}

impl File {
    pub fn new(name: &str) -> Self {
        let mut file = Self::default();

        file.records.Assembly.push(rec::Assembly {
            Name: file.strings.insert(name),
            HashAlgId: 0x00008004,
            MajorVersion: 0xFF,
            MinorVersion: 0xFF,
            BuildNumber: 0xFF,
            RevisionNumber: 0xFF,
            Flags: AssemblyFlags::WindowsRuntime,
            ..Default::default()
        });

        file.records.Module.push(rec::Module {
            Name: file.strings.insert(name),
            Mvid: 1,
            ..Default::default()
        });

        // Some parsers require the `mscorlib` reference implied by "System" types.
        file.AssemblyRef("System");

        file.TypeDef("", "<Module>", TypeDefOrRef::default(), TypeAttributes(0));

        file
    }

    /// Sets the reference index used to resolve external `TypeRef` scopes.
    pub fn set_reference(&mut self, reference: reader::Index) {
        self.reference = Some(reference);
    }

    pub(crate) fn set_reference_assemblies(
        &mut self,
        references: BTreeMap<(String, String), AssemblyRefIdentity>,
    ) {
        self.reference_assemblies = references.into_iter().collect();
    }

    #[cfg(test)]
    pub(crate) fn set_assembly_identity(&mut self, identity: AssemblyIdentity) {
        let public_key = self.blobs.insert(&identity.public_key).0;
        let name = self.strings.insert(&identity.name);
        let culture = self.strings.insert(&identity.culture).0;
        let assembly = &mut self.records.Assembly[0];
        assembly.MajorVersion = identity.major_version;
        assembly.MinorVersion = identity.minor_version;
        assembly.BuildNumber = identity.build_number;
        assembly.RevisionNumber = identity.revision_number;
        assembly.Flags = AssemblyFlags(identity.flags);
        assembly.PublicKey = public_key;
        assembly.Name = name;
        assembly.Culture = culture;
    }

    pub fn reference(&self) -> Option<&reader::Index> {
        self.reference.as_ref()
    }

    fn ModuleRef(&mut self, name: &str) -> ModuleRef {
        if let Some(pos) = self.ModuleRef.get(name) {
            return *pos;
        }

        let pos = ModuleRef(self.records.ModuleRef.push_pos(rec::ModuleRef {
            Name: self.strings.insert(name),
        }));

        self.ModuleRef.insert(name.to_string(), pos);
        pos
    }

    pub fn ImplMap(
        &mut self,
        method: MethodDef,
        flags: PInvokeAttributes,
        import_name: &str,
        import_scope: &str,
    ) {
        let scope = self.ModuleRef(import_scope);

        self.records.ImplMap.push(rec::ImplMap {
            MappingFlags: flags,
            MemberForwarded: MemberForwarded::MethodDef(method),
            ImportName: self.strings.insert(import_name),
            ImportScope: scope,
        });
    }

    fn AssemblyRef(&mut self, assembly_name: &str) -> AssemblyRef {
        self.assembly_ref(AssemblyRefIdentity::named(assembly_name))
    }

    fn assembly_ref(&mut self, identity: AssemblyRefIdentity) -> AssemblyRef {
        if let Some(pos) = self.AssemblyRef.get(&identity) {
            return *pos;
        }

        let pos = AssemblyRef(self.records.AssemblyRef.push_pos(rec::AssemblyRef {
            MajorVersion: identity.major_version,
            MinorVersion: identity.minor_version,
            BuildNumber: identity.build_number,
            RevisionNumber: identity.revision_number,
            Flags: AssemblyFlags(identity.flags),
            PublicKeyOrToken: self.blobs.insert(&identity.public_key_or_token),
            Name: self.strings.insert(&identity.name),
            Culture: self.strings.insert(&identity.culture).0,
            HashValue: self.blobs.insert(&identity.hash_value).0,
        }));

        self.AssemblyRef.insert(identity, pos);
        pos
    }

    pub fn TypeDef(
        &mut self,
        namespace: &str,
        name: &str,
        extends: TypeDefOrRef,
        flags: TypeAttributes,
    ) -> TypeDef {
        if !flags.is_nested() {
            self.local_types
                .insert((namespace.to_string(), name.to_string()));
        }

        TypeDef(self.records.TypeDef.push_pos(rec::TypeDef {
            TypeName: self.strings.insert(name),
            TypeNamespace: self.strings.insert(namespace),
            Flags: flags,
            Extends: extends,
            FieldList: self.records.Field.len() as u32,
            MethodList: self.records.MethodDef.len() as u32,
        }))
    }

    pub fn TypeRef(&mut self, namespace: &str, name: &str) -> TypeRef {
        self.type_ref(namespace, name, false)
    }

    /// Creates a reference to a compiler-known core-library type.
    ///
    /// An exact supplied reference definition takes precedence. Otherwise this uses the same
    /// `mscorlib` identity as the legacy `System` sentinel.
    pub fn CoreTypeRef(&mut self, namespace: &str, name: &str) -> TypeRef {
        self.type_ref(namespace, name, true)
    }

    fn type_ref(&mut self, namespace: &str, name: &str, core: bool) -> TypeRef {
        if let Some(reference) = self
            .TypeRef
            .get(namespace)
            .and_then(|names| names.get(name))
            .copied()
        {
            if core && !name.contains('/') {
                let assembly = self
                    .reference_assembly(namespace, name)
                    .unwrap_or_else(AssemblyRefIdentity::system);
                self.records.TypeRef[reference.0 as usize].ResolutionScope =
                    ResolutionScope::AssemblyRef(self.assembly_ref(assembly));
            }
            if core {
                self.core_type_refs.insert(reference);
            }
            return reference;
        }

        let pos = if let Some((parent, leaf)) = name.rsplit_once('/') {
            let enclosing = self.type_ref(namespace, parent, core);
            TypeRef(self.records.TypeRef.push_pos(rec::TypeRef {
                TypeName: self.strings.insert(leaf),
                TypeNamespace: self.strings.insert(""),
                ResolutionScope: ResolutionScope::TypeRef(enclosing),
            }))
        } else {
            let reference = self.reference_assembly(namespace, name);
            let scope = if core {
                ResolutionScope::AssemblyRef(
                    self.assembly_ref(reference.unwrap_or_else(AssemblyRefIdentity::system)),
                )
            } else if self.has_local_type(namespace, name) {
                ResolutionScope::Module(Module(0))
            } else if let Some(reference) = reference {
                ResolutionScope::AssemblyRef(self.assembly_ref(reference))
            } else if namespace == "System" {
                ResolutionScope::AssemblyRef(self.AssemblyRef("System"))
            } else {
                ResolutionScope::Module(Module(0))
            };

            TypeRef(self.records.TypeRef.push_pos(rec::TypeRef {
                TypeName: self.strings.insert(name),
                TypeNamespace: self.strings.insert(namespace),
                ResolutionScope: scope,
            }))
        };

        self.TypeRef
            .entry(namespace.to_string())
            .or_default()
            .insert(name.to_string(), pos);
        if core {
            self.core_type_refs.insert(pos);
        }

        pos
    }

    fn reference_assembly(&self, namespace: &str, name: &str) -> Option<AssemblyRefIdentity> {
        self.reference_assemblies
            .get(&(namespace.to_string(), name.to_string()))
            .cloned()
            .or_else(|| {
                self.reference
                    .as_ref()
                    .and_then(|reference| reference.exact_type(namespace, name))
                    .and_then(|ty| ty.assembly())
                    .map(AssemblyRefIdentity::from_definition)
            })
    }

    fn has_local_type(&self, namespace: &str, name: &str) -> bool {
        self.local_types
            .iter()
            .any(|(local_namespace, local_name)| local_namespace == namespace && local_name == name)
    }

    fn localize_inferred_type_refs(&mut self) {
        for (namespace, names) in &self.TypeRef {
            for (name, reference) in names {
                if !self.core_type_refs.contains(reference)
                    && !name.contains('/')
                    && self.has_local_type(namespace, name)
                {
                    self.records.TypeRef[reference.0 as usize].ResolutionScope =
                        ResolutionScope::Module(Module(0));
                }
            }
        }
    }

    pub fn TypeSpec(&mut self, namespace: &str, name: &str, generics: &[Type]) -> TypeSpec {
        debug_assert!(!generics.is_empty());
        // Avoid doubling an existing generic arity suffix read from a winmd.
        let base = name.split_once('`').map_or(name, |(base, _)| base);
        let name = format!("{base}`{}", generics.len());
        let type_ref = self.TypeRef(namespace, &name);

        let mut buffer = vec![];
        buffer.push(ELEMENT_TYPE_GENERICINST);
        buffer.push(ELEMENT_TYPE_CLASS);
        buffer.write_compressed(TypeDefOrRef::TypeRef(type_ref).encode() as usize);
        buffer.write_compressed(generics.len());

        for ty in generics {
            self.Type(ty, &mut buffer);
        }

        let signature = self.blobs.insert(&buffer);

        if let Some(pos) = self.TypeSpec.get(&signature) {
            return *pos;
        }

        let pos = TypeSpec(self.records.TypeSpec.push_pos(rec::TypeSpec {
            Signature: signature,
        }));
        self.TypeSpec.insert(signature, pos);
        pos
    }

    pub fn Field(&mut self, name: &str, ty: &Type, flags: FieldAttributes) -> Field {
        let signature = self.FieldSig(ty);

        Field(self.records.Field.push_pos(rec::Field {
            Name: self.strings.insert(name),
            Flags: flags,
            Signature: signature,
        }))
    }

    pub fn MethodDef(
        &mut self,
        name: &str,
        signature: &Signature,
        flags: MethodAttributes,
        impl_flags: MethodImplAttributes,
    ) -> MethodDef {
        let signature = self.MethodDefSig(signature);

        MethodDef(self.records.MethodDef.push_pos(rec::MethodDef {
            RVA: 0,
            ImplFlags: impl_flags,
            Flags: flags,
            Name: self.strings.insert(name),
            Signature: signature,
            ParamList: self.records.Param.len() as u32,
        }))
    }

    pub fn MemberRef(
        &mut self,
        name: &str,
        signature: &Signature,
        parent: MemberRefParent,
    ) -> MemberRef {
        let signature = self.MethodDefSig(signature);

        let record = rec::MemberRef {
            Name: self.strings.insert(name),
            Signature: signature,
            Parent: parent,
        };

        if let Some(pos) = self.MemberRef.get(&record) {
            return *pos;
        }

        let pos = MemberRef(self.records.MemberRef.push_pos(record));
        self.MemberRef.insert(record, pos);
        pos
    }

    /// Adds a `Param` row to the file, returning the row offset.
    pub fn Param(&mut self, name: &str, sequence: u16, flags: ParamAttributes) -> Param {
        Param(self.records.Param.push_pos(rec::Param {
            Flags: flags,
            Sequence: sequence,
            Name: self.strings.insert(name),
        }))
    }

    /// Adds a `Property` row to the file, returning the row offset.
    pub fn Property(&mut self, name: &str, ty: &Type) -> Property {
        let signature = self.PropertySig(ty);

        Property(self.records.Property.push_pos(rec::Property {
            Flags: 0,
            Name: self.strings.insert(name),
            Type: signature,
        }))
    }

    /// Adds a `PropertyMap` row associating a type with its first property.
    pub fn PropertyMap(&mut self, parent: TypeDef, property_list: Property) -> PropertyMap {
        PropertyMap(self.records.PropertyMap.push_pos(rec::PropertyMap {
            Parent: parent,
            PropertyList: property_list,
        }))
    }

    /// Adds an `Event` row to the file, returning the row offset. `ty` is the event's
    /// handler delegate type.
    pub fn Event(&mut self, name: &str, ty: &Type) -> Event {
        let Type::ClassName(ty) = ty else {
            panic!("invalid event type");
        };
        let event_type = TypeDefOrRef::TypeRef(self.TypeRef(&ty.namespace, &ty.name));

        Event(self.records.Event.push_pos(rec::Event {
            Flags: 0,
            Name: self.strings.insert(name),
            EventType: event_type,
        }))
    }

    /// Adds an `EventMap` row associating a type with its first event.
    pub fn EventMap(&mut self, parent: TypeDef, event_list: Event) -> EventMap {
        EventMap(self.records.EventMap.push_pos(rec::EventMap {
            Parent: parent,
            EventList: event_list,
        }))
    }

    /// Adds a `MethodSemantics` row linking an accessor method to a property or event.
    pub fn MethodSemantics(
        &mut self,
        semantics: u16,
        method: MethodDef,
        association: HasSemantics,
    ) -> MethodSemantics {
        MethodSemantics(self.records.MethodSemantics.push_pos(rec::MethodSemantics {
            Semantics: semantics,
            Method: method,
            Association: association,
        }))
    }

    /// Adds an `Attribute` row without assigning its sorted row offset.
    pub fn Attribute(
        &mut self,
        parent: HasAttribute,
        ty: AttributeType,
        value: &[(String, Value)],
    ) {
        let value = self.AttributeValue(value, 0x53, None);

        self.Attribute
            .entry(parent)
            .or_default()
            .push(rec::Attribute {
                Parent: parent,
                Type: ty,
                Value: value,
            });
    }

    pub fn AttributeWithNamedProperties(
        &mut self,
        parent: HasAttribute,
        ty: AttributeType,
        value: &[(String, Value)],
    ) {
        let value = self.AttributeValue(value, 0x54, None);

        self.Attribute
            .entry(parent)
            .or_default()
            .push(rec::Attribute {
                Parent: parent,
                Type: ty,
                Value: value,
            });
    }

    pub fn AttributeWithNamedArgKinds(
        &mut self,
        parent: HasAttribute,
        ty: AttributeType,
        value: &[(String, Value)],
        named_arg_kinds: &[u8],
    ) {
        let value = self.AttributeValue(value, 0x53, Some(named_arg_kinds));

        self.Attribute
            .entry(parent)
            .or_default()
            .push(rec::Attribute {
                Parent: parent,
                Type: ty,
                Value: value,
            });
    }

    pub fn Constant(&mut self, parent: HasConstant, value: &Value) {
        let ty = value.ty().code();
        let value = self.ConstantValue(value);

        self.Constant.insert(
            parent,
            rec::Constant {
                Parent: parent,
                Type: ty,
                Value: value,
            },
        );
    }

    pub fn GenericParam(
        &mut self,
        name: &str,
        owner: TypeOrMethodDef,
        number: u16,
        flags: GenericParamAttributes,
    ) {
        self.GenericParam
            .entry(owner)
            .or_default()
            .push(rec::GenericParam {
                Name: self.strings.insert(name),
                Number: number,
                Owner: owner,
                Flags: flags,
            });
    }

    pub fn ClassLayout(&mut self, parent: TypeDef, packing_size: u16, class_size: u32) {
        self.records.ClassLayout.push(rec::ClassLayout {
            PackingSize: packing_size,
            ClassSize: class_size,
            Parent: parent.0,
        });
    }

    pub fn FieldLayout(&mut self, field: Field, offset: u32) {
        self.records.FieldLayout.push(rec::FieldLayout {
            Offset: offset,
            Field: field.0,
        });
    }

    pub fn NestedClass(&mut self, inner: TypeDef, outer: TypeDef) {
        debug_assert!(inner.0 > outer.0);

        self.records.NestedClass.push(rec::NestedClass {
            NestedClass: inner.0,
            EnclosingClass: outer.0,
        });
    }

    pub fn InterfaceImpl(&mut self, class: TypeDef, interface: &Type) -> InterfaceImpl {
        let Type::ClassName(interface) = interface else {
            panic!("invalid interface type");
        };

        let interface = if interface.generics.is_empty() {
            TypeDefOrRef::TypeRef(self.TypeRef(&interface.namespace, &interface.name))
        } else {
            TypeDefOrRef::TypeSpec(self.TypeSpec(
                &interface.namespace,
                &interface.name,
                &interface.generics,
            ))
        };

        InterfaceImpl(self.records.InterfaceImpl.push_pos(rec::InterfaceImpl {
            Class: class,
            Interface: interface,
        }))
    }

    /// Encodes the `Type` into the buffer, adding any required `TypeRef` rows to the file.
    fn Type(&mut self, ty: &Type, buffer: &mut Vec<u8>) {
        match ty {
            Type::Void => buffer.push(ELEMENT_TYPE_VOID),
            Type::Bool => buffer.push(ELEMENT_TYPE_BOOLEAN),
            Type::Char => buffer.push(ELEMENT_TYPE_CHAR),
            Type::I8 => buffer.push(ELEMENT_TYPE_I1),
            Type::U8 => buffer.push(ELEMENT_TYPE_U1),
            Type::I16 => buffer.push(ELEMENT_TYPE_I2),
            Type::U16 => buffer.push(ELEMENT_TYPE_U2),
            Type::I32 => buffer.push(ELEMENT_TYPE_I4),
            Type::U32 => buffer.push(ELEMENT_TYPE_U4),
            Type::I64 => buffer.push(ELEMENT_TYPE_I8),
            Type::U64 => buffer.push(ELEMENT_TYPE_U8),
            Type::F32 => buffer.push(ELEMENT_TYPE_R4),
            Type::F64 => buffer.push(ELEMENT_TYPE_R8),
            Type::ISize => buffer.push(ELEMENT_TYPE_I),
            Type::USize => buffer.push(ELEMENT_TYPE_U),
            Type::String => buffer.push(ELEMENT_TYPE_STRING),
            Type::Object => buffer.push(ELEMENT_TYPE_OBJECT),

            Type::Array(ty) => {
                buffer.push(ELEMENT_TYPE_SZARRAY);
                self.Type(ty, buffer);
            }

            Type::RefMut(ty) => {
                buffer.push(ELEMENT_TYPE_BYREF);
                self.Type(ty, buffer);
            }

            Type::RefConst(ty) => {
                buffer.write_compressed(ELEMENT_TYPE_CMOD_REQD as usize);
                let pos = self.CoreTypeRef("System.Runtime.CompilerServices", "IsConst");
                buffer.write_compressed(TypeDefOrRef::TypeRef(pos).encode() as usize);
                buffer.push(ELEMENT_TYPE_BYREF);
                self.Type(ty, buffer);
            }

            Type::PtrMut(ty, pointers) => {
                for _ in 0..*pointers {
                    buffer.write_compressed(ELEMENT_TYPE_PTR as usize);
                }

                self.Type(ty, buffer);
            }

            Type::PtrConst(ty, pointers) => {
                buffer.write_compressed(ELEMENT_TYPE_CMOD_REQD as usize);
                let pos = self.CoreTypeRef("System.Runtime.CompilerServices", "IsConst");
                buffer.write_compressed(TypeDefOrRef::TypeRef(pos).encode() as usize);

                for _ in 0..*pointers {
                    buffer.write_compressed(ELEMENT_TYPE_PTR as usize);
                }

                self.Type(ty, buffer);
            }

            Type::ArrayFixed(ty, len) => {
                // See II.23.2.13 ArrayShape
                buffer.push(ELEMENT_TYPE_ARRAY);
                self.Type(ty, buffer);
                buffer.write_compressed(1); // rank
                buffer.write_compressed(1); // num_sizes
                buffer.write_compressed(*len); // size
                buffer.write_compressed(0); // num_lo_bounds
            }

            Type::Generic(_, number) => {
                buffer.push(ELEMENT_TYPE_VAR);
                buffer.write_compressed((*number).into());
            }

            Type::ClassName(ty) => {
                self.TypeName(false, &ty.namespace, &ty.name, &ty.generics, buffer);
            }
            Type::ValueName(ty) => {
                self.TypeName(true, &ty.namespace, &ty.name, &ty.generics, buffer);
            }
        }
    }

    fn TypeName(
        &mut self,
        is_value_type: bool,
        namespace: &str,
        name: &str,
        generics: &[Type],
        buffer: &mut Vec<u8>,
    ) {
        let pos = if !generics.is_empty() {
            buffer.push(ELEMENT_TYPE_GENERICINST);
            // Strip any existing `N suffix before re-deriving it (see TypeSpec).
            let base = name.split_once('`').map_or(name, |(base, _)| base);
            let name = format!("{base}`{}", generics.len());
            self.TypeRef(namespace, &name)
        } else {
            self.TypeRef(namespace, name)
        };

        buffer.push(if is_value_type {
            ELEMENT_TYPE_VALUETYPE
        } else {
            ELEMENT_TYPE_CLASS
        });
        buffer.write_compressed(TypeDefOrRef::TypeRef(pos).encode() as usize);

        if !generics.is_empty() {
            buffer.write_compressed(generics.len());

            for ty in generics {
                self.Type(ty, buffer);
            }
        }
    }

    /// Writes the `Type` into a `FileSig` buffer and stores it in the file, returning the blob
    /// offset.
    fn FieldSig(&mut self, ty: &Type) -> BlobId {
        let mut buffer = vec![0x6]; // FIELD
        self.Type(ty, &mut buffer);
        self.blobs.insert(&buffer)
    }

    fn PropertySig(&mut self, ty: &Type) -> BlobId {
        let mut buffer = vec![0x28]; // HASTHIS | PROPERTY
        buffer.write_compressed(0); // parameter count
        self.Type(ty, &mut buffer);
        self.blobs.insert(&buffer)
    }

    /// Stores a method signature and returns its blob offset.
    fn MethodDefSig(&mut self, signature: &Signature) -> BlobId {
        let mut buffer = vec![signature.flags.0];
        buffer.write_compressed(signature.types.len());
        self.Type(&signature.return_type, &mut buffer);

        for ty in &signature.types {
            self.Type(ty, &mut buffer);
        }

        self.blobs.insert(&buffer)
    }

    fn ConstantValue(&mut self, value: &Value) -> BlobId {
        let mut buffer = vec![];
        buffer.write_value(value);
        self.blobs.insert(&buffer)
    }

    fn AttributeValue(
        &mut self,
        values: &[(String, Value)],
        named_arg_kind: u8,
        named_arg_kinds: Option<&[u8]>,
    ) -> BlobId {
        let mut buffer = vec![];
        buffer.write_u16(1); // prolog

        let mut count = 0;

        for (name, value) in values {
            if name.is_empty() {
                count += 1;
                buffer.write_value(value);
            } else {
                break;
            }
        }

        buffer.write_u16((values.len() - count).try_into().unwrap());
        if let Some(kinds) = named_arg_kinds {
            assert_eq!(kinds.len(), values.len() - count);
            assert!(kinds.iter().all(|kind| matches!(kind, 0x53 | 0x54)));
        }

        for (index, (name, value)) in values[count..].iter().enumerate() {
            buffer.push(named_arg_kinds.map_or(named_arg_kind, |kinds| kinds[index]));

            if let Value::EnumValue(tn, _) = value {
                // SERIALIZATION_TYPE_ENUM (ECMA-335 II.23.1.16): 0x55 followed by
                // a SerString of the fully-qualified enum type name.
                buffer.push(0x55);
                let enum_name = if tn.namespace.is_empty() {
                    tn.name.clone()
                } else {
                    format!("{}.{}", tn.namespace, tn.name)
                };
                buffer.write_compressed(enum_name.len());
                buffer.extend_from_slice(enum_name.as_bytes());
            } else {
                buffer.push(value.ty().code());
            }

            buffer.write_compressed(name.len());
            buffer.extend_from_slice(name.as_bytes());
            buffer.write_value(value);
        }

        self.blobs.insert(&buffer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(value: &str) -> Vec<u8> {
        let mut chunks = value.as_bytes().chunks_exact(2);
        let result = chunks
            .by_ref()
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect();
        assert!(chunks.remainder().is_empty());
        result
    }

    #[test]
    fn public_key_token_matches_framework_strong_name() {
        let public_key = hex(concat!(
            "0024000004800000940000000602000000240000525341310004000001000100",
            "07d1fa57c4aed9f0a32e84aa0faefd0de9e8fd6aec8f87fb03766c834c99921e",
            "b23be79ad9d5dcc1dd9ad236132102900b723cf980957fc4e177108fc607774f",
            "29e8320e92ea05ece4e821c0a5efe8f1645c4c0c93c1ab99285d622caa652c1d",
            "fad63d745d6f2de5f17e5eaf0fc4963d261c8a12436518206dc093344d5ad293",
        ));
        assert_eq!(public_key.len(), 160);
        assert_eq!(public_key_token(&public_key), hex("b03f5f7f11d50a3a"));
    }

    #[test]
    fn sha1_handles_padding_boundaries() {
        let cases = [
            (55, "8ae2d46729cfe68ff927af5eec9c7d1b66d65ac2"),
            (56, "636e2ec698dac903498e648bd2f3af641d3c88cb"),
            (63, "6d942da0c4392b123528f2905c713a3ce28364bd"),
            (64, "c6138d514ffa2135bfce0ed0b8fac65669917ec7"),
        ];

        for (length, expected) in cases {
            let input: Vec<_> = (0..length)
                .map(|value| u8::try_from(value).unwrap())
                .collect();
            assert_eq!(sha1(&input).as_slice(), hex(expected), "length {length}");
        }
    }
}
