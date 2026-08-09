use super::*;

pub(super) fn parse_recovering<T: syn::parse::Parse>(
    input: syn::parse::ParseStream,
    errors: &mut Vec<syn::Error>,
) -> Option<T> {
    let start = input.cursor().span().start();
    match input.parse::<T>() {
        Ok(item) => Some(item),
        Err(error) => {
            errors.push(error);
            recover(input, start);
            None
        }
    }
}

pub(super) fn recover(input: syn::parse::ParseStream, start: proc_macro2::LineColumn) {
    if input.is_empty() {
        return;
    }

    let current = input.cursor().span().start();
    if current == start {
        let _ = input.parse::<proc_macro2::TokenTree>();
    }

    while !input.is_empty() && !starts_item(input) {
        let Ok(token) = input.parse::<proc_macro2::TokenTree>() else {
            break;
        };
        if matches!(token, proc_macro2::TokenTree::Punct(ref punct) if punct.as_char() == ';') {
            break;
        }
    }
}

fn starts_item(input: syn::parse::ParseStream) -> bool {
    input.peek(syn::Token![#])
        || input.peek(syn::Token![use])
        || input.peek(syn::Token![struct])
        || input.peek(syn::Token![enum])
        || input.peek(syn::Token![mod])
        || input.peek(interface)
        || input.peek(attribute)
        || input.peek(syn::Token![union])
        || input.peek(syn::Token![extern])
        || input.peek(syn::Token![const])
        || input.peek(delegate)
        || input.peek(class)
        || input.peek(syn::Token![type])
}

pub(super) fn parse_arch_bitmask(expr: &syn::Expr) -> Option<i32> {
    match expr {
        syn::Expr::Path(p)
            if p.qself.is_none()
                && p.path.leading_colon.is_none()
                && p.path.segments.len() == 1 =>
        {
            arch_name_to_bits(&p.path.segments[0].ident.to_string())
        }
        syn::Expr::Binary(syn::ExprBinary {
            left,
            op: syn::BinOp::BitOr(_),
            right,
            ..
        }) => {
            let l = parse_arch_bitmask(left)?;
            let r = parse_arch_bitmask(right)?;
            Some(l | r)
        }
        _ => None,
    }
}

fn arch_name_to_bits(name: &str) -> Option<i32> {
    match name {
        "X86" => Some(1),
        "X64" => Some(2),
        "Arm64" => Some(4),
        _ => None,
    }
}

/// Parses a GUID integer with decimal or hexadecimal syntax and optional separators.
pub(super) fn parse_guid_u128(lit: &syn::LitInt) -> Result<u128, ()> {
    let s: String = lit
        .token()
        .to_string()
        .chars()
        .filter(|&c| c != '_')
        .collect();
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u128::from_str_radix(hex, 16).map_err(|_| ())
    } else {
        s.parse::<u128>().map_err(|_| ())
    }
}

pub(crate) fn make_sig(
    fn_token: syn::Token![fn],
    ident: syn::Ident,
    generics: syn::Generics,
    paren_token: syn::token::Paren,
    inputs: syn::punctuated::Punctuated<syn::FnArg, syn::Token![,]>,
    variadic: Option<syn::Variadic>,
    output: syn::ReturnType,
) -> syn::Signature {
    syn::Signature {
        constness: None,
        asyncness: None,
        unsafety: None,
        abi: None,
        fn_token,
        ident,
        generics,
        paren_token,
        inputs,
        variadic,
        output,
    }
}

pub(crate) fn parse_fn_inputs(
    content: &syn::parse::ParseBuffer,
) -> syn::Result<(
    syn::punctuated::Punctuated<syn::FnArg, syn::Token![,]>,
    Option<syn::Variadic>,
)> {
    let mut args = syn::punctuated::Punctuated::new();
    let mut variadic = None;

    while !content.is_empty() {
        let fork = content.fork();
        let _ = fork.call(syn::Attribute::parse_outer);
        if fork.peek(syn::Token![...]) {
            let attrs = content.call(syn::Attribute::parse_outer)?;
            let dots: syn::Token![...] = content.parse()?;
            variadic = Some(syn::Variadic {
                attrs,
                pat: None,
                dots,
                comma: if content.is_empty() {
                    None
                } else {
                    Some(content.parse()?)
                },
            });
            break;
        }

        let arg: syn::FnArg = content.parse()?;
        args.push_value(arg);

        if content.is_empty() {
            break;
        }

        let comma: syn::Token![,] = content.parse()?;
        args.push_punct(comma);
    }

    Ok((args, variadic))
}

/// Parses `-> #[attr]* Type`, keeping return-value attributes with the return type.
pub(crate) fn parse_return_type_with_attrs(
    input: syn::parse::ParseStream,
) -> syn::Result<(syn::ReturnType, Vec<syn::Attribute>)> {
    if input.peek(syn::Token![->]) {
        let arrow = input.parse::<syn::Token![->]>()?;
        let return_attrs = input.call(syn::Attribute::parse_outer)?;
        let ty: syn::Type = input.parse()?;
        Ok((syn::ReturnType::Type(arrow, Box::new(ty)), return_attrs))
    } else {
        Ok((syn::ReturnType::Default, vec![]))
    }
}

pub(super) trait IdentMethods {
    fn unraw_to_string(&self) -> String;
}

impl IdentMethods for syn::Ident {
    fn unraw_to_string(&self) -> String {
        use syn::ext::IdentExt;
        self.unraw().to_string()
    }
}
