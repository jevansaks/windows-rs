use super::*;

#[derive(Debug)]
pub struct Module {
    pub attrs: Vec<syn::Attribute>,
    pub token: syn::Token![mod],
    pub name: syn::Ident,
    pub items: Vec<Item>,
}

impl syn::parse::Parse for Module {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let attrs = input.call(syn::Attribute::parse_outer)?;
        let token = input.parse()?;
        let name = input.parse()?;

        let content;
        syn::braced!(content in input);
        let mut items = vec![];

        while !content.is_empty() {
            items.push(content.parse()?);
        }

        Ok(Self {
            attrs,
            token,
            name,
            items,
        })
    }
}

impl std::fmt::Display for Module {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let name = self.name.to_string();
        if name
            .strip_prefix('_')
            .is_some_and(|name| name.as_bytes().first().is_some_and(u8::is_ascii_digit))
        {
            name[1..].fmt(f)
        } else {
            name.fmt(f)
        }
    }
}
