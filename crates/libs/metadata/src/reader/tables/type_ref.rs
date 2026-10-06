use super::*;

impl std::fmt::Debug for TypeRef<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "TypeRef({}.{})", self.namespace(), self.name())
    }
}

impl<'a> TypeRef<'a> {
    pub fn scope(&self) -> ResolutionScope<'a> {
        self.decode(0)
    }

    pub fn name(&self) -> &'a str {
        self.str(1)
    }

    pub fn namespace(&self) -> &'a str {
        self.str(2)
    }

    /// Gets the logical type name, including the full enclosing TypeRef path.
    pub fn qualified_name(&self) -> TypeName {
        if let ResolutionScope::TypeRef(enclosing) = self.scope() {
            let mut name = enclosing.qualified_name();
            name.name.push('/');
            name.name.push_str(self.name());
            name
        } else {
            TypeName::named(self.namespace(), self.name())
        }
    }
}
