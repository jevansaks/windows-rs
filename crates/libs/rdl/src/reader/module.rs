use super::*;

#[derive(Debug)]
pub struct Module {
    pub attrs: Vec<syn::Attribute>,
    pub token: syn::Token![mod],
    pub name: syn::Ident,
    pub items: Vec<Item>,
    pub syntax_errors: Vec<syn::Error>,
}

impl syn::parse::Parse for Module {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let attrs = input.call(syn::Attribute::parse_outer)?;
        let token = input.parse()?;
        let name = input.parse()?;

        let content;
        syn::braced!(content in input);
        let mut items = vec![];
        let mut syntax_errors = vec![];

        while !content.is_empty() {
            if let Some(item) = parse_recovering::<Item>(&content, &mut syntax_errors) {
                items.push(item);
            }
        }

        Ok(Self {
            attrs,
            token,
            name,
            items,
            syntax_errors,
        })
    }
}

impl std::fmt::Display for Module {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        self.name.fmt(f)
    }
}
