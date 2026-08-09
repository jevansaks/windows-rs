use super::*;

/// Rewrites a flat winmd into header-based namespaces for package generation.
#[derive(Default)]
pub struct Remapper {
    input: Vec<PathBuf>,
    output: PathBuf,
    routes: HashMap<String, String>,
    sources: Vec<String>,
    fallback: String,
}

impl Remapper {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn input(&mut self, input: impl AsRef<Path>) -> &mut Self {
        self.input.push(input.as_ref().to_path_buf());
        self
    }

    pub fn inputs<I, S>(&mut self, inputs: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<Path>,
    {
        for input in inputs {
            self.input(input);
        }
        self
    }

    /// Registers a namespace whose members are remapped.
    pub fn source(&mut self, namespace: &str) -> &mut Self {
        self.sources.push(namespace.to_string());
        self
    }

    /// Registers namespaces whose members are remapped.
    pub fn sources<I, S>(&mut self, namespaces: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for namespace in namespaces {
            self.source(namespace.as_ref());
        }
        self
    }

    pub fn fallback(&mut self, namespace: &str) -> &mut Self {
        self.fallback = namespace.to_string();
        self
    }

    pub fn route(&mut self, name: impl Into<String>, namespace: impl Into<String>) -> &mut Self {
        self.routes.insert(name.into(), namespace.into());
        self
    }

    pub fn routes<I, K, V>(&mut self, routes: I) -> &mut Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        for (name, namespace) in routes {
            self.route(name, namespace);
        }
        self
    }

    pub fn output(&mut self, output: impl AsRef<Path>) -> &mut Self {
        self.output = output.as_ref().to_path_buf();
        self
    }

    pub fn remap(&self) -> Result<(), Error> {
        if self.output.as_os_str().is_empty() {
            return Err(Error::new("output is required"));
        }

        let name = self
            .output
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| {
                Error::new(format!("invalid output path `{}`", self.output.display()))
            })?;

        let files = read_inputs(&self.input)?;
        let index = reader::Index::new(files);
        let mut file = writer::File::new(name);
        let mut context = CopyContext::default();

        // Split source `Apis` containers after all regular types have been written.
        let mut apis: Vec<reader::TypeDef> = Vec::new();
        let mut types: Vec<reader::TypeDef> = index.types().collect();
        types.sort_by(|a, b| (a.namespace(), a.name()).cmp(&(b.namespace(), b.name())));

        for ty in types {
            if self.is_source_apis(ty) {
                apis.push(ty);
            } else {
                copy::write_type(
                    self,
                    &mut file,
                    &mut context,
                    &index,
                    ty,
                    None,
                    copy::TypeOptions {
                        arch_override: None,
                        invoke_signature: None,
                    },
                );
            }
        }

        self.split_apis(&mut file, &mut context, &apis);

        context.finish(&mut file)?;
        let file = file.finish();
        validate_output(&file)?;
        std::fs::write(&self.output, file.bytes())
            .map_err(|e| Error::new(format!("failed to write `{}`: {e}", self.output.display())))
    }

    fn is_source_apis(&self, ty: reader::TypeDef) -> bool {
        !ty.flags().contains(TypeAttributes::WindowsRuntime)
            && ty.category() == reader::TypeCategory::Class
            && ty.name() == "Apis"
            && self.sources.iter().any(|s| s == ty.namespace())
    }

    fn target(&self, namespace: &str, name: &str) -> String {
        if self.sources.iter().any(|s| s == namespace) {
            self.routes
                .get(trim_tick(name))
                .cloned()
                .unwrap_or_else(|| self.fallback.clone())
        } else {
            namespace.to_string()
        }
    }

    fn remap_type(&self, ty: &Type) -> Type {
        match ty {
            Type::ClassName(tn) => Type::ClassName(self.remap_type_name(tn)),
            Type::ValueName(tn) => Type::ValueName(self.remap_type_name(tn)),
            Type::Array(inner) => Type::Array(Box::new(self.remap_type(inner))),
            Type::RefMut(inner) => Type::RefMut(Box::new(self.remap_type(inner))),
            Type::RefConst(inner) => Type::RefConst(Box::new(self.remap_type(inner))),
            Type::PtrMut(inner, n) => Type::PtrMut(Box::new(self.remap_type(inner)), *n),
            Type::PtrConst(inner, n) => Type::PtrConst(Box::new(self.remap_type(inner)), *n),
            Type::ArrayFixed(inner, n) => Type::ArrayFixed(Box::new(self.remap_type(inner)), *n),
            other => other.clone(),
        }
    }

    fn remap_type_name(&self, tn: &TypeName) -> TypeName {
        TypeName {
            namespace: self.target(&tn.namespace, &tn.name),
            name: tn.name.clone(),
            generics: tn.generics.iter().map(|g| self.remap_type(g)).collect(),
        }
    }

    /// Splits flat `Apis` containers while preserving each TypeDef's contiguous member range.
    fn split_apis(
        &self,
        file: &mut writer::File,
        context: &mut CopyContext,
        apis: &[reader::TypeDef],
    ) {
        let mut namespaces: Vec<String> = Vec::new();
        let mut fields: HashMap<String, Vec<reader::Field>> = HashMap::new();
        let mut methods: HashMap<String, Vec<reader::MethodDef>> = HashMap::new();

        let record = |namespaces: &mut Vec<String>, namespace: String| {
            if !namespaces.contains(&namespace) {
                namespaces.push(namespace);
            }
        };

        let template = apis.first().copied();

        for &container in apis {
            for field in container.fields() {
                let namespace = self.target(container.namespace(), field.name());
                record(&mut namespaces, namespace.clone());
                fields.entry(namespace).or_default().push(field);
            }
            for method in container.methods() {
                let namespace = self.target(container.namespace(), method.name());
                record(&mut namespaces, namespace.clone());
                methods.entry(namespace).or_default().push(method);
            }
        }

        namespaces.sort();

        let Some(template) = template else { return };

        for namespace in &namespaces {
            let extends = template
                .extends()
                .map(|extends| {
                    let namespace = self.target(extends.namespace(), extends.name());
                    writer::TypeDefOrRef::TypeRef(file.TypeRef(&namespace, extends.name()))
                })
                .unwrap_or_default();
            let type_def = file.TypeDef(namespace, "Apis", extends, template.flags());

            for field in fields.get(namespace).into_iter().flatten() {
                copy::write_field(self, file, *field, None);
            }

            for method in methods.get(namespace).into_iter().flatten() {
                copy::write_method(self, file, context, *method, &[], None, None);
            }

            write_attributes(file, writer::HasAttribute::TypeDef(type_def), template);
        }
    }
}

impl copy::CopyPolicy for Remapper {
    fn namespace(&self, def: reader::TypeDef) -> String {
        self.target(def.namespace(), def.name())
    }

    fn ty(&self, ty: &Type) -> Type {
        self.remap_type(ty)
    }
}
