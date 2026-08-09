mod attribute;
mod attribute_ref;
mod callback;
mod class;
mod compile;
mod r#const;
mod delegate;
mod r#enum;
mod field;
mod file;
mod r#fn;
pub(crate) mod guid;
mod index;
mod interface;
mod item;
mod method;
mod module;
mod origin;
mod param;
mod projection;
mod resolver;
mod source;
mod r#struct;
mod syntax;
mod typedef;
mod union;
mod validate;

use super::*;
use attribute::*;
use callback::*;
use class::*;
pub use compile::Reader;
use r#const::*;
use delegate::*;
use r#enum::*;
use field::*;
use file::*;
use r#fn::*;
use index::*;
use interface::*;
use item::*;
use method::*;
use module::*;
use origin::*;
use param::*;
use projection::*;
use resolver::*;
pub(crate) use source::{item_names, parse_source};
use r#struct::*;
use syntax::*;
use typedef::*;
use union::*;
use windows_metadata as metadata;

fn fixed_signed_value(value: i64) -> metadata::Value {
    i32::try_from(value)
        .map(metadata::Value::I32)
        .unwrap_or(metadata::Value::I64(value))
}

fn fixed_unsigned_value(value: u64) -> metadata::Value {
    u32::try_from(value)
        .map(metadata::Value::U32)
        .unwrap_or(metadata::Value::U64(value))
}

fn encode(
    assembly_name: &str,
    index: &Index,
    reference: metadata::reader::Index,
) -> Result<(Vec<u8>, Vec<Error>), Error> {
    let mut output = metadata::writer::File::new(assembly_name);
    output.set_reference(reference);
    let mut origins = OriginMap::default();
    let mut projections = ProjectionContext::default();

    for (namespace, members) in &index.namespaces {
        for variants in members.types.values() {
            for (file, item) in variants {
                let name = item.to_string();
                let encoder = &mut Encoder {
                    output: &mut output,
                    index,
                    file,
                    namespace,
                    name: &name,
                    generics: vec![],
                    generic_args: vec![],
                    origins: &mut origins,
                    projections: &mut projections,
                };
                match item {
                    Item::Attribute(ty) => encoder.encode_attribute(ty),
                    Item::Callback(ty) => encoder.encode_callback(ty),
                    Item::Class(ty) => encoder.encode_class(ty),
                    Item::Const(ty) => encoder.encode_const(ty),
                    Item::Delegate(ty) => encoder.encode_delegate(ty),
                    Item::Enum(ty) => encoder.encode_enum(ty),
                    Item::Fn(ty) => encoder.encode_fn(ty),
                    Item::Interface(ty) => encoder.encode_interface(ty),
                    Item::Struct(ty) => encoder.encode_struct(ty),
                    Item::Typedef(ty) => encoder.encode_typedef(ty),
                    Item::Union(ty) => encoder.encode_union(ty),
                    Item::Module(_) => unreachable!(
                        "Module items are expanded during indexing and never encoded directly"
                    ),
                }?;
            }
        }

        if !members.functions.is_empty() || !members.constants.is_empty() {
            let class = metadata::writer::TypeDefOrRef::TypeRef(output.TypeRef("System", "Object"));

            output.TypeDef(
                namespace,
                "Apis",
                class,
                metadata::TypeAttributes::Public | metadata::TypeAttributes::Sealed,
            );

            for (name, variants) in &members.functions {
                for (file, item) in variants {
                    let Item::Fn(ty) = item else {
                        unreachable!("functions index only contains Item::Fn")
                    };
                    Encoder {
                        output: &mut output,
                        index,
                        file,
                        namespace,
                        name,
                        generics: vec![],
                        generic_args: vec![],
                        origins: &mut origins,
                        projections: &mut projections,
                    }
                    .encode_fn(ty)?;
                }
            }

            for (name, variants) in &members.constants {
                for (file, item) in variants {
                    let Item::Const(ty) = item else {
                        unreachable!("constants index only contains Item::Const")
                    };
                    Encoder {
                        output: &mut output,
                        index,
                        file,
                        namespace,
                        name,
                        generics: vec![],
                        generic_args: vec![],
                        origins: &mut origins,
                        projections: &mut projections,
                    }
                    .encode_const(ty)?;
                }
            }
        }
    }

    let output = output.finalize();
    let validation = output
        .validate_inferred()
        .into_iter()
        .map(|error| origins.error(error))
        .collect::<Vec<_>>();
    let bytes = if validation.is_empty() {
        output.into_stream()
    } else {
        vec![]
    };

    Ok((bytes, validation))
}

struct Encoder<'a> {
    output: &'a mut metadata::writer::File,
    origins: &'a mut OriginMap,
    index: &'a Index<'a>,
    file: &'a File,
    namespace: &'a str,
    name: &'a str,
    generics: Vec<String>,
    generic_args: Vec<metadata::Type>,
    projections: &'a mut ProjectionContext,
}

#[derive(Default)]
struct ProjectionContext {
    methods: Vec<ProjectedMethod>,
}

struct ProjectedMethod {
    owner: metadata::TypeName,
    name: String,
    method: metadata::writer::MethodDef,
}

impl ProjectionContext {
    fn insert(
        &mut self,
        owner: &metadata::TypeName,
        name: &str,
        method: metadata::writer::MethodDef,
    ) {
        self.methods.push(ProjectedMethod {
            owner: owner.clone(),
            name: name.to_string(),
            method,
        });
    }

    fn get(&self, owner: &metadata::TypeName, name: &str) -> Option<metadata::writer::MethodDef> {
        self.methods
            .iter()
            .find(|method| method.owner == *owner && method.name == name)
            .map(|method| method.method)
    }
}

impl Encoder<'_> {
    fn origin<H: metadata::writer::RowHandle, S: Spanned>(&mut self, handle: H, source: &S) {
        self.origins.insert(handle, self.file, source);
    }

    fn resolver(&self) -> Resolver<'_, '_> {
        Resolver {
            index: self.index,
            reference: self.output.reference().unwrap(),
            file: self.file,
            namespace: self.namespace,
            generics: &self.generics,
            generic_args: &self.generic_args,
        }
    }

    fn error<S: Spanned>(&self, spanned: S, message: &str) -> Error {
        let start = spanned.span().start();

        Error::new(message, &self.file.source, start.line, start.column)
            .with_source_id(self.file.source_id)
    }

    fn err<T, S: Spanned>(&self, spanned: S, message: &str) -> Result<T, Error> {
        Err(self.error(spanned, message))
    }

    fn read_packed(&self, attrs: &[syn::Attribute]) -> Result<Option<u16>, Error> {
        for attr in attrs {
            if !attr.path().is_ident("packed") {
                continue;
            }

            let Ok(size_literal) = attr.parse_args::<syn::LitInt>() else {
                return self.err(attr, "`packed` attribute requires an integer argument");
            };

            let Ok(size) = size_literal.base10_parse::<u16>() else {
                return self.err(attr, "`packed` size must be a valid u16");
            };

            return Ok(Some(size));
        }

        Ok(None)
    }

    /// Reads forced over-alignment from `#[align(N)]`.
    fn read_align(&self, attrs: &[syn::Attribute]) -> Result<Option<u16>, Error> {
        for attr in attrs {
            if !attr.path().is_ident("align") {
                continue;
            }

            let Ok(size_literal) = attr.parse_args::<syn::LitInt>() else {
                return self.err(attr, "`align` attribute requires an integer argument");
            };

            let Ok(size) = size_literal.base10_parse::<u16>() else {
                return self.err(attr, "`align` size must be a valid u16");
            };

            return Ok(Some(size));
        }

        Ok(None)
    }

    fn read_arch(&self, attrs: &[syn::Attribute]) -> Result<Option<i32>, Error> {
        for attr in attrs {
            if !attr.path().is_ident("arch") {
                continue;
            }

            let expr = attr.parse_args::<syn::Expr>().map_err(|_| {
                self.error(
                    attr,
                    "`arch` attribute requires architecture arguments (e.g. `#[arch(X86)]`)",
                )
            })?;

            let bits = parse_arch_bitmask(&expr).ok_or_else(|| {
                self.error(
                    attr,
                    "invalid `arch` value; expected `X86`, `X64`, `Arm64`, or a `|`-combination",
                )
            })?;

            return Ok(Some(bits));
        }

        Ok(None)
    }

    fn encode_type(&self, ty: &syn::Type) -> Result<metadata::Type, Error> {
        self.resolver().resolve_type(ty)
    }

    fn encode_value(
        &self,
        ty: &metadata::Type,
        value: &syn::Expr,
    ) -> Result<metadata::Value, Error> {
        if matches!(ty, metadata::Type::ISize | metadata::Type::USize)
            && let Some(value) = self.encode_fixed_width_value(value)?
        {
            return Ok(value);
        }

        let value = match ty {
            metadata::Type::Char => metadata::Value::Char(self.encode_lit_uint(value, 16)? as u16),
            metadata::Type::I8 => metadata::Value::I8(self.encode_lit_sint(value, 8)? as i8),
            metadata::Type::U8 => metadata::Value::U8(self.encode_lit_uint(value, 8)? as u8),
            metadata::Type::I16 => metadata::Value::I16(self.encode_lit_sint(value, 16)? as i16),
            metadata::Type::U16 => metadata::Value::U16(self.encode_lit_uint(value, 16)? as u16),
            metadata::Type::I32 => metadata::Value::I32(self.encode_lit_sint(value, 32)? as i32),
            metadata::Type::U32 => metadata::Value::U32(self.encode_lit_uint(value, 32)? as u32),
            metadata::Type::I64 => metadata::Value::I64(self.encode_lit_sint(value, 64)?),
            metadata::Type::U64 => metadata::Value::U64(self.encode_lit_uint(value, 64)?),
            metadata::Type::F32 => metadata::Value::F32(self.encode_neg_lit_float::<f32>(value)?),
            metadata::Type::F64 => metadata::Value::F64(self.encode_neg_lit_float::<f64>(value)?),
            metadata::Type::String => metadata::Value::Utf16(self.encode_lit_string(value)?),
            metadata::Type::ISize => fixed_signed_value(self.encode_lit_sint(value, 64)?),
            metadata::Type::USize => fixed_unsigned_value(self.encode_lit_uint(value, 64)?),
            metadata::Type::PtrMut(_, _) | metadata::Type::PtrConst(_, _) => {
                let v = self.encode_neg_lit_int::<i64>(value)?;
                if let Ok(v) = i32::try_from(v) {
                    metadata::Value::I32(v)
                } else {
                    metadata::Value::I64(v)
                }
            }

            metadata::Type::ValueName(tn) | metadata::Type::ClassName(tn) => {
                let underlying = self
                    .output
                    .reference()
                    .and_then(|r| r.get(&tn.namespace, &tn.name).next())
                    .and_then(|def| def.underlying_type())
                    .or_else(|| self.rdl_underlying_type(&tn.namespace, &tn.name));

                match underlying {
                    Some(underlying) => return self.encode_value(&underlying, value),
                    None => {
                        return self.err(value, &format!("constant type not supported: {ty:?}"));
                    }
                }
            }
            rest => return self.err(value, &format!("constant type not supported: {rest:?}")),
        };

        Ok(value)
    }

    fn encode_fixed_width_value(
        &self,
        value: &syn::Expr,
    ) -> Result<Option<metadata::Value>, Error> {
        let int = match value {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Int(int),
                ..
            }) => int,
            syn::Expr::Unary(syn::ExprUnary { expr, .. }) => {
                let syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Int(int),
                    ..
                }) = expr.as_ref()
                else {
                    return Ok(None);
                };
                int
            }
            _ => return Ok(None),
        };

        let value = match int.suffix() {
            "i32" => metadata::Value::I32(self.encode_lit_sint(value, 32)? as i32),
            "u32" => metadata::Value::U32(self.encode_lit_uint(value, 32)? as u32),
            "i64" => metadata::Value::I64(self.encode_lit_sint(value, 64)?),
            "u64" => metadata::Value::U64(self.encode_lit_uint(value, 64)?),
            _ => return Ok(None),
        };
        Ok(Some(value))
    }

    fn rdl_underlying_type(&self, namespace: &str, name: &str) -> Option<metadata::Type> {
        let item = self.index.get(namespace, name).next()?;

        match item {
            Item::Typedef(t) => self.encode_underlying(&t.ty, namespace),
            Item::Enum(e) => {
                // Enum-typed constants encode against the enum's `#[repr(iN)]` type.
                let repr = e.attrs.iter().find(|a| a.path().is_ident("repr"))?;
                let path = repr.parse_args::<syn::Path>().ok()?;
                self.encode_path(&path).ok()
            }
            Item::Struct(s) => {
                let mut fields = s.fields.iter();

                if let Some(field) = fields.next()
                    && fields.next().is_none()
                    && let FieldType::Type(ty) = &field.ty
                {
                    return self.encode_underlying(ty, namespace);
                }

                None
            }
            _ => None,
        }
    }

    /// Resolves a typedef's bare sibling names in the typedef namespace, not the constant namespace.
    fn encode_underlying(&self, ty: &syn::Type, namespace: &str) -> Option<metadata::Type> {
        match ty {
            // MAKEINTRESOURCE-style pointer constants only need the pointer kind.
            syn::Type::Ptr(ptr) => {
                let pointee = self
                    .encode_underlying(&ptr.elem, namespace)
                    .unwrap_or(metadata::Type::Void);
                Some(if ptr.mutability.is_some() {
                    metadata::Type::PtrMut(Box::new(pointee), 1)
                } else {
                    metadata::Type::PtrConst(Box::new(pointee), 1)
                })
            }
            syn::Type::Path(tp)
                if tp.qself.is_none()
                    && tp.path.segments.len() == 1
                    && matches!(tp.path.segments[0].arguments, syn::PathArguments::None) =>
            {
                if let Ok(resolved) = self.encode_type(ty)
                    && !matches!(
                        resolved,
                        metadata::Type::ValueName(_) | metadata::Type::ClassName(_)
                    )
                {
                    return Some(resolved);
                }
                let ident = tp.path.segments[0].ident.unraw_to_string();
                Some(metadata::Type::value_named(namespace, &ident))
            }
            _ => self.encode_type(ty).ok(),
        }
    }

    fn encode_neg_lit_int<T>(&self, expr: &syn::Expr) -> Result<T, Error>
    where
        T: std::str::FromStr + TryFrom<i128>,
        T::Err: std::fmt::Display,
    {
        let value = match expr {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Int(int),
                ..
            }) => int.base10_parse().ok(),
            syn::Expr::Unary(syn::ExprUnary {
                op: syn::UnOp::Neg(_),
                expr,
                ..
            }) => match expr.as_ref() {
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Int(int),
                    ..
                }) => int
                    .base10_parse::<u64>()
                    .ok()
                    .and_then(|v| T::try_from(-(v as i128)).ok()),
                _ => None,
            },
            _ => None,
        };

        value.ok_or_else(|| self.error(expr, "value not valid"))
    }

    fn encode_lit_int<T>(&self, expr: &syn::Expr) -> Result<T, Error>
    where
        T: std::str::FromStr,
        T::Err: std::fmt::Display,
    {
        let value = match expr {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Int(int),
                ..
            }) => int.base10_parse().ok(),

            _ => None,
        };

        value.ok_or_else(|| self.error(expr, "value not valid"))
    }

    /// Accepts C unsigned sentinels spelled as negated casts by masking to the target width.
    fn encode_lit_uint(&self, expr: &syn::Expr, bits: u32) -> Result<u64, Error> {
        let mask: u128 = if bits >= 128 {
            u128::MAX
        } else {
            (1u128 << bits) - 1
        };
        let value = match expr {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Int(int),
                ..
            }) => int.base10_parse::<u64>().ok(),
            syn::Expr::Unary(syn::ExprUnary {
                op: syn::UnOp::Neg(_),
                expr,
                ..
            }) => match expr.as_ref() {
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Int(int),
                    ..
                }) => int
                    .base10_parse::<u64>()
                    .ok()
                    .map(|v| ((v as i128).wrapping_neg() as u128 & mask) as u64),
                _ => None,
            },
            _ => None,
        };

        value.ok_or_else(|| self.error(expr, "value not valid"))
    }

    /// Reinterprets signed constants from their C bit pattern, including overflowing HRESULTs.
    fn encode_lit_sint(&self, expr: &syn::Expr, bits: u32) -> Result<i64, Error> {
        let raw: Option<u64> = match expr {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Int(int),
                ..
            }) => int.base10_parse::<u64>().ok(),
            syn::Expr::Unary(syn::ExprUnary {
                op: syn::UnOp::Neg(_),
                expr,
                ..
            }) => match expr.as_ref() {
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Int(int),
                    ..
                }) => int
                    .base10_parse::<u64>()
                    .ok()
                    .map(|v| (v as i128).wrapping_neg() as u64),
                _ => None,
            },
            _ => None,
        };

        let raw = raw.ok_or_else(|| self.error(expr, "value not valid"))?;

        if bits >= 64 {
            Ok(raw as i64)
        } else {
            let mask = (1u64 << bits) - 1;
            let masked = raw & mask;
            let sign_bit = 1u64 << (bits - 1);
            Ok(if masked & sign_bit != 0 {
                (masked | !mask) as i64
            } else {
                masked as i64
            })
        }
    }

    fn encode_neg_lit_float<T>(&self, expr: &syn::Expr) -> Result<T, Error>
    where
        T: std::str::FromStr + std::ops::Neg<Output = T>,
        T::Err: std::fmt::Display,
    {
        let value = match expr {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Float(float),
                ..
            }) => float.base10_parse().ok(),
            syn::Expr::Unary(syn::ExprUnary {
                op: syn::UnOp::Neg(_),
                expr,
                ..
            }) => match expr.as_ref() {
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Float(float),
                    ..
                }) => float.base10_parse().ok().map(|value: T| -value),
                _ => None,
            },
            _ => None,
        };

        value.ok_or_else(|| self.error(expr, "value not valid"))
    }

    fn encode_lit_string(&self, expr: &syn::Expr) -> Result<String, Error> {
        let value = match expr {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(string),
                ..
            }) => Some(string.value()),
            _ => None,
        };

        value.ok_or_else(|| self.error(expr, "value not valid"))
    }

    fn encode_path(&self, ty: &syn::Path) -> Result<metadata::Type, Error> {
        self.resolver().resolve_path(ty)
    }

    fn encode_return_type(&self, ty: &syn::ReturnType) -> Result<metadata::Type, Error> {
        match ty {
            syn::ReturnType::Type(_, ty) => self.encode_type(ty),
            _ => Ok(metadata::Type::Void),
        }
    }

    /// Rejects references from WinRT types to non-WinRT named types.
    fn validate_type_is_winrt<S: Spanned + quote::ToTokens>(
        &self,
        span: &S,
        ty: &metadata::Type,
    ) -> Result<(), Error> {
        match ty {
            metadata::Type::ValueName(tn) | metadata::Type::ClassName(tn) => {
                for generic_ty in &tn.generics {
                    self.validate_type_is_winrt(span, generic_ty)?;
                }

                if let Some(is_winrt) = self.index.is_winrt(&tn.namespace, &tn.name) {
                    if !is_winrt {
                        return self.err(span, "WinRT types cannot refer to non-WinRT types");
                    }
                } else if let Some(reference) = self.output.reference()
                    && let Some(def) = reference.get(&tn.namespace, &tn.name).next()
                    && !def
                        .flags()
                        .contains(metadata::TypeAttributes::WindowsRuntime)
                {
                    return self.err(span, "WinRT types cannot refer to non-WinRT types");
                }
            }
            metadata::Type::PtrMut(inner, _) | metadata::Type::PtrConst(inner, _) => {
                self.validate_type_is_winrt(span, inner)?;
            }
            metadata::Type::RefMut(inner) | metadata::Type::RefConst(inner) => {
                self.validate_type_is_winrt(span, inner)?;
            }
            metadata::Type::Array(inner) | metadata::Type::ArrayFixed(inner, _) => {
                self.validate_type_is_winrt(span, inner)?;
            }
            _ => {}
        }

        Ok(())
    }
}
