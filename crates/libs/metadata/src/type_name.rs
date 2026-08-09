use super::*;

#[derive(Debug, PartialEq, Clone)]
pub struct TypeName {
    pub namespace: String,
    pub name: String,
    pub generics: Vec<Type>,
}

impl TypeName {
    pub fn named(namespace: &str, name: &str) -> Self {
        Self {
            namespace: namespace.to_string(),
            name: name.to_string(),
            generics: vec![],
        }
    }

    pub(crate) fn serialized_name(&self) -> String {
        let mut result = if self.namespace.is_empty() {
            self.name.clone()
        } else {
            format!("{}.{}", self.namespace, self.name)
        };
        if !self.generics.is_empty() {
            result.push('<');
            for (index, ty) in self.generics.iter().enumerate() {
                if index != 0 {
                    result.push(',');
                }
                write_serialized_type(&mut result, ty);
            }
            result.push('>');
        }
        result
    }

    pub(crate) fn from_serialized_name(value: &str) -> Option<Self> {
        let mut parser = SerializedTypeParser { value, offset: 0 };
        let (Type::ClassName(result) | Type::ValueName(result)) = parser.parse_type()? else {
            return None;
        };
        (parser.offset == value.len()).then_some(result)
    }
}

impl PartialEq<(&str, &str)> for &TypeName {
    fn eq(&self, other: &(&str, &str)) -> bool {
        self.namespace == other.0 && self.name == other.1
    }
}

fn write_serialized_type(result: &mut String, ty: &Type) {
    let name = match ty {
        Type::Bool => "Boolean",
        Type::Char => "Char16",
        Type::I8 => "Int8",
        Type::U8 => "UInt8",
        Type::I16 => "Int16",
        Type::U16 => "UInt16",
        Type::I32 => "Int32",
        Type::U32 => "UInt32",
        Type::I64 => "Int64",
        Type::U64 => "UInt64",
        Type::F32 => "Single",
        Type::F64 => "Double",
        Type::ISize => "IntPtr",
        Type::USize => "UIntPtr",
        Type::String => "String",
        Type::Object => "Object",
        Type::ClassName(name) | Type::ValueName(name) => {
            result.push_str(&name.serialized_name());
            return;
        }
        _ => {
            result.push_str(&format!("{ty:?}"));
            return;
        }
    };
    result.push_str(name);
}

struct SerializedTypeParser<'a> {
    value: &'a str,
    offset: usize,
}

impl SerializedTypeParser<'_> {
    fn parse_type(&mut self) -> Option<Type> {
        let start = self.offset;
        while let Some(byte) = self.value.as_bytes().get(self.offset)
            && !matches!(byte, b'<' | b'>' | b',')
        {
            self.offset += 1;
        }
        let name = self.value.get(start..self.offset)?;
        let primitive = match name {
            "Boolean" | "System.Boolean" => Some(Type::Bool),
            "Char16" | "System.Char" => Some(Type::Char),
            "Int8" | "System.SByte" => Some(Type::I8),
            "UInt8" | "System.Byte" => Some(Type::U8),
            "Int16" | "System.Int16" => Some(Type::I16),
            "UInt16" | "System.UInt16" => Some(Type::U16),
            "Int32" | "System.Int32" => Some(Type::I32),
            "UInt32" | "System.UInt32" => Some(Type::U32),
            "Int64" | "System.Int64" => Some(Type::I64),
            "UInt64" | "System.UInt64" => Some(Type::U64),
            "Single" | "System.Single" => Some(Type::F32),
            "Double" | "System.Double" => Some(Type::F64),
            "IntPtr" | "System.IntPtr" => Some(Type::ISize),
            "UIntPtr" | "System.UIntPtr" => Some(Type::USize),
            "String" | "System.String" => Some(Type::String),
            "Object" | "System.Object" => Some(Type::Object),
            _ => None,
        };
        if let Some(primitive) = primitive {
            return Some(primitive);
        }

        let (namespace, name) = name
            .rfind('.')
            .map_or(("", name), |dot| (&name[..dot], &name[dot + 1..]));
        if name.is_empty() {
            return None;
        }
        let mut result = TypeName::named(namespace, name);
        if self.value.as_bytes().get(self.offset) == Some(&b'<') {
            self.offset += 1;
            loop {
                result.generics.push(self.parse_type()?);
                match self.value.as_bytes().get(self.offset) {
                    Some(b',') => self.offset += 1,
                    Some(b'>') => {
                        self.offset += 1;
                        break;
                    }
                    _ => return None,
                }
            }
        }
        Some(Type::ClassName(result))
    }
}
