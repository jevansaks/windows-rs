use super::*;

#[derive(Debug)]
pub struct File {
    pub items: Vec<Item>,
    pub imports: Vec<Import>,
    pub source: String,
    pub source_id: SourceId,
    pub(super) syntax_errors: Vec<syn::Error>,
}

#[derive(Debug)]
pub struct Import {
    pub path: Vec<String>,
    pub local: Option<String>,
    pub glob: bool,
    pub span: Span,
    pub used: std::cell::Cell<bool>,
    pub shadowed: std::cell::Cell<bool>,
    pub allow_unused: bool,
    pub allow_shadowed: bool,
}

#[derive(Clone, Copy, Default)]
struct ImportPolicy {
    allow_unused: bool,
    allow_shadowed: bool,
}

impl syn::parse::Parse for File {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut items = vec![];
        let mut imports = vec![];
        let mut syntax_errors = vec![];
        while !input.is_empty() {
            if peek_use(input) {
                let start = input.cursor().span().start();
                match input.parse::<syn::ItemUse>() {
                    Ok(item) => {
                        let result = import_policy(&item.attrs).and_then(|policy| {
                            collect_imports(&item.tree, &mut vec![], &mut imports, policy)
                        });
                        if let Err(error) = result {
                            syntax_errors.push(error);
                        }
                    }
                    Err(error) => {
                        syntax_errors.push(error);
                        recover(input, start);
                    }
                }
            } else if let Some(module) = parse_recovering::<Module>(input, &mut syntax_errors) {
                items.push(Item::Module(module));
            }
        }

        Ok(Self {
            items,
            imports,
            source: String::new(),
            source_id: SourceId::UNKNOWN,
            syntax_errors,
        })
    }
}

impl File {
    pub(super) fn take_syntax_errors(&mut self) -> Vec<syn::Error> {
        let mut errors = std::mem::take(&mut self.syntax_errors);
        for item in &mut self.items {
            item.take_syntax_errors(&mut errors);
        }
        errors
    }
}

fn peek_use(input: syn::parse::ParseStream) -> bool {
    let fork = input.fork();
    fork.call(syn::Attribute::parse_outer).is_ok() && fork.peek(syn::Token![use])
}

fn collect_imports(
    tree: &syn::UseTree,
    prefix: &mut Vec<String>,
    imports: &mut Vec<Import>,
    policy: ImportPolicy,
) -> syn::Result<()> {
    use syn::spanned::Spanned;

    match tree {
        syn::UseTree::Path(path) => {
            let name = path.ident.unraw_to_string();
            if prefix.is_empty() && matches!(name.as_str(), "crate" | "self" | "super") {
                return Err(syn::Error::new(
                    path.ident.span(),
                    "RDL imports must use an absolute metadata namespace",
                ));
            }
            prefix.push(name);
            collect_imports(&path.tree, prefix, imports, policy)?;
            prefix.pop();
        }
        syn::UseTree::Name(name) => {
            let name = name.ident.unraw_to_string();
            let path = if name == "self" {
                if prefix.is_empty() {
                    return Err(syn::Error::new(
                        tree.span(),
                        "`self` import requires a namespace prefix",
                    ));
                }
                prefix.clone()
            } else {
                let mut path = prefix.clone();
                path.push(name);
                path
            };
            imports.push(Import {
                local: path.last().cloned(),
                path,
                glob: false,
                span: tree.span(),
                used: std::cell::Cell::new(false),
                shadowed: std::cell::Cell::new(false),
                allow_unused: policy.allow_unused,
                allow_shadowed: policy.allow_shadowed,
            });
        }
        syn::UseTree::Rename(rename) => {
            let mut path = prefix.clone();
            let name = rename.ident.unraw_to_string();
            if name != "self" {
                path.push(name);
            } else if path.is_empty() {
                return Err(syn::Error::new(
                    rename.ident.span(),
                    "`self` import requires a namespace prefix",
                ));
            }
            imports.push(Import {
                path,
                local: Some(rename.rename.unraw_to_string()),
                glob: false,
                span: tree.span(),
                used: std::cell::Cell::new(false),
                shadowed: std::cell::Cell::new(false),
                allow_unused: policy.allow_unused,
                allow_shadowed: policy.allow_shadowed,
            });
        }
        syn::UseTree::Glob(_) => {
            if prefix.is_empty() {
                return Err(syn::Error::new(
                    tree.span(),
                    "glob import requires a namespace",
                ));
            }
            imports.push(Import {
                path: prefix.clone(),
                local: None,
                glob: true,
                span: tree.span(),
                used: std::cell::Cell::new(false),
                shadowed: std::cell::Cell::new(false),
                allow_unused: policy.allow_unused,
                allow_shadowed: policy.allow_shadowed,
            });
        }
        syn::UseTree::Group(group) => {
            for tree in &group.items {
                collect_imports(tree, prefix, imports, policy)?;
            }
        }
    }
    Ok(())
}

fn import_policy(attrs: &[syn::Attribute]) -> syn::Result<ImportPolicy> {
    let mut policy = ImportPolicy::default();
    for attr in attrs {
        if !attr.path().is_ident("allow") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("unused_imports") {
                policy.allow_unused = true;
                Ok(())
            } else if meta.path.is_ident("shadowed_imports") {
                policy.allow_shadowed = true;
                Ok(())
            } else {
                Err(meta.error("unsupported RDL warning suppression"))
            }
        })?;
    }
    Ok(policy)
}

#[test]
fn recovers_module_items_after_syntax_error() {
    let mut file: File = syn::parse_str(
        r#"
#[win32]
mod Test {
    this is not valid rdl
    struct Value {
        field: i32,
        field: i32,
    }
}
"#,
    )
    .unwrap();

    assert_eq!(file.take_syntax_errors().len(), 1);
    let Item::Module(module) = &file.items[0] else {
        unreachable!()
    };
    assert_eq!(module.items.len(), 1);
}
