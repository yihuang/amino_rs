mod map;
mod message;
mod oneof;
mod scalar;

use std::fmt;
use std::slice;

use failure::Error;
use proc_macro2::TokenStream;
use syn::{Attribute, Ident, Lit, LitBool, Meta, MetaList, MetaNameValue, NestedMeta};

use super::compute_disfix;

#[derive(Clone)]
pub enum Field {
    /// A scalar field.
    Scalar(scalar::Field),
    /// A message field.
    Message(message::Field),
    /// A map field.
    Map(map::Field),
    /// A oneof field.
    Oneof(oneof::Field),
}

impl Field {
    /// Creates a new `Field` from an iterator of field attributes.
    ///
    /// If the meta items are invalid, an error will be returned.
    /// If the field should be ignored, `None` is returned.
    pub fn new(attrs: Vec<Attribute>, inferred_tag: Option<u32>) -> Result<Option<Field>, Error> {
        let attrs = prost_attrs(attrs)?;

        // TODO: check for ignore attribute.

        let field = if let Some(field) = scalar::Field::new(&attrs, inferred_tag)? {
            Field::Scalar(field)
        } else if let Some(field) = message::Field::new(&attrs, inferred_tag)? {
            Field::Message(field)
        } else if let Some(field) = map::Field::new(&attrs, inferred_tag)? {
            Field::Map(field)
        } else if let Some(field) = oneof::Field::new(&attrs)? {
            Field::Oneof(field)
        } else {
            bail!("no type attribute");
        };

        Ok(Some(field))
    }

    /// Creates a new oneof `Field` from an iterator of field attributes.
    ///
    /// If the meta items are invalid, an error will be returned.
    /// If the field should be ignored, `None` is returned.
    pub fn new_oneof(attrs: Vec<Attribute>) -> Result<Option<Field>, Error> {
        let attrs = prost_attrs(attrs)?;

        // TODO: check for ignore attribute.

        let field = if let Some(field) = scalar::Field::new_oneof(&attrs)? {
            Field::Scalar(field)
        } else if let Some(field) = message::Field::new_oneof(&attrs)? {
            Field::Message(field)
        } else if let Some(field) = map::Field::new_oneof(&attrs)? {
            Field::Map(field)
        } else {
            bail!("no type attribute for oneof field");
        };

        Ok(Some(field))
    }

    pub fn tags(&self) -> Vec<u32> {
        match *self {
            Field::Scalar(ref scalar) => vec![scalar.tag],
            Field::Message(ref message) => vec![message.tag],
            Field::Map(ref map) => vec![map.tag],
            Field::Oneof(ref oneof) => oneof.tags.clone(),
        }
    }

    /// Returns a statement which encodes the field.
    pub fn encode(&self, ident: TokenStream) -> TokenStream {
        match *self {
            Field::Scalar(ref scalar) => scalar.encode(ident),
            Field::Message(ref message) => message.encode(ident),
            Field::Map(ref map) => map.encode(ident),
            Field::Oneof(ref oneof) => oneof.encode(ident),
        }
    }

    /// Returns an expression which evaluates to the result of merging a decoded
    /// value into the field.
    pub fn merge(&self, ident: TokenStream) -> TokenStream {
        match *self {
            Field::Scalar(ref scalar) => scalar.merge(ident),
            Field::Message(ref message) => message.merge(ident),
            Field::Map(ref map) => map.merge(ident),
            Field::Oneof(ref oneof) => oneof.merge(ident),
        }
    }

    /// Returns an expression which evaluates to the encoded length of the field.
    pub fn encoded_len(&self, ident: TokenStream) -> TokenStream {
        match *self {
            Field::Scalar(ref scalar) => scalar.encoded_len(ident),
            Field::Map(ref map) => map.encoded_len(ident),
            Field::Message(ref msg) => msg.encoded_len(ident),
            Field::Oneof(ref oneof) => oneof.encoded_len(ident),
        }
    }

    /// Returns a statement which clears the field.
    pub fn clear(&self, ident: TokenStream) -> TokenStream {
        match *self {
            Field::Scalar(ref scalar) => scalar.clear(ident),
            Field::Message(ref message) => message.clear(ident),
            Field::Map(ref map) => map.clear(ident),
            Field::Oneof(ref oneof) => oneof.clear(ident),
        }
    }

    pub fn default(&self) -> TokenStream {
        match *self {
            Field::Scalar(ref scalar) => scalar.default(),
            _ => quote!(::std::default::Default::default()),
        }
    }

    /// Produces the fragment implementing debug for the given field.
    pub fn debug(&self, ident: TokenStream) -> TokenStream {
        match *self {
            Field::Scalar(ref scalar) => {
                let wrapper = scalar.debug(quote!(ScalarWrapper));
                quote! {
                    {
                        #wrapper
                        ScalarWrapper(&#ident)
                    }
                }
            }
            Field::Map(ref map) => {
                let wrapper = map.debug(quote!(MapWrapper));
                quote! {
                    {
                        #wrapper
                        MapWrapper(&#ident)
                    }
                }
            }
            _ => quote!(&#ident),
        }
    }

    pub fn methods(&self, ident: &Ident) -> Option<TokenStream> {
        match *self {
            Field::Scalar(ref scalar) => scalar.methods(ident),
            Field::Map(ref map) => map.methods(ident),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Label {
    /// An optional field.
    Optional,
    /// A required field.
    Required,
    /// A repeated field.
    Repeated,
}

impl Label {
    fn as_str(&self) -> &'static str {
        match *self {
            Label::Optional => "optional",
            Label::Required => "required",
            Label::Repeated => "repeated",
        }
    }

    fn variants() -> slice::Iter<'static, Label> {
        const VARIANTS: &'static [Label] = &[Label::Optional, Label::Required, Label::Repeated];
        VARIANTS.iter()
    }

    /// Parses a string into a field label.
    /// If the string doesn't match a field label, `None` is returned.
    fn from_attr(attr: &Meta) -> Option<Label> {
        if let Meta::Word(ref ident) = *attr {
            for &label in Label::variants() {
                if ident == label.as_str() {
                    return Some(label);
                }
            }
        }
        None
    }
}

impl fmt::Debug for Label {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Display for Label {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Get the items belonging to the 'prost' list attribute, e.g. `#[prost(foo, bar="baz")]`.
fn prost_attrs(attrs: Vec<Attribute>) -> Result<Vec<Meta>, Error> {
    Ok(attrs
        .iter()
        .flat_map(Attribute::interpret_meta)
        .flat_map(|meta| match meta {
            Meta::List(MetaList { ident, nested, .. }) => {
                if ident == "prost_amino" {
                    nested.into_iter().collect()
                } else {
                    Vec::new()
                }
            }
            _ => Vec::new(),
        })
        .flat_map(|attr| -> Result<_, _> {
            match attr {
                NestedMeta::Meta(attr) => Ok(attr),
                NestedMeta::Literal(lit) => bail!("invalid prost_amino attribute: {:?}", lit),
            }
        })
        .collect())
}

pub fn set_option<T>(option: &mut Option<T>, value: T, message: &str) -> Result<(), Error>
where
    T: fmt::Debug,
{
    if let Some(ref existing) = *option {
        bail!("{}: {:?} and {:?}", message, existing, value);
    }
    *option = Some(value);
    Ok(())
}

pub fn set_bool(b: &mut bool, message: &str) -> Result<(), Error> {
    if *b {
        bail!("{}", message);
    } else {
        *b = true;
        Ok(())
    }
}

/// Unpacks an attribute into a (key, boolean) pair, returning the boolean value.
/// If the key doesn't match the attribute, `None` is returned.
fn bool_attr(key: &str, attr: &Meta) -> Result<Option<bool>, Error> {
    if attr.name() != key {
        return Ok(None);
    }
    match *attr {
        Meta::Word(..) => Ok(Some(true)),
        Meta::List(ref meta_list) => {
            // TODO(rustlang/rust#23121): slice pattern matching would make this much nicer.
            if meta_list.nested.len() == 1 {
                if let NestedMeta::Literal(Lit::Bool(LitBool { value, .. })) = meta_list.nested[0] {
                    return Ok(Some(value));
                }
            }
            bail!("invalid {} attribute", key);
        }
        Meta::NameValue(MetaNameValue {
            lit: Lit::Str(ref lit),
            ..
        }) => lit
            .value()
            .parse::<bool>()
            .map_err(Error::from)
            .map(Option::Some),
        Meta::NameValue(MetaNameValue {
            lit: Lit::Bool(LitBool { value, .. }),
            ..
        }) => Ok(Some(value)),
        _ => bail!("invalid {} attribute", key),
    }
}

/// Checks if an attribute matches a word.
fn word_attr(key: &str, attr: &Meta) -> bool {
    if let Meta::Word(ref ident) = *attr {
        ident == key
    } else {
        false
    }
}

fn tag_attr(attr: &Meta) -> Result<Option<u32>, Error> {
    if attr.name() != "tag" {
        return Ok(None);
    }
    match *attr {
        Meta::List(ref meta_list) => {
            // TODO(rustlang/rust#23121): slice pattern matching would make this much nicer.
            if meta_list.nested.len() == 1 {
                if let NestedMeta::Literal(Lit::Int(ref lit)) = meta_list.nested[0] {
                    return Ok(Some(lit.value() as u32));
                }
            }
            bail!("invalid tag attribute: {:?}", attr);
        }
        Meta::NameValue(ref meta_name_value) => match meta_name_value.lit {
            Lit::Str(ref lit) => lit
                .value()
                .parse::<u32>()
                .map_err(Error::from)
                .map(Option::Some),
            Lit::Int(ref lit) => Ok(Some(lit.value() as u32)),
            _ => bail!("invalid tag attribute: {:?}", attr),
        },
        _ => bail!("invalid tag attribute: {:?}", attr),
    }
}

fn amino_name_attr(attr: &Meta) -> Result<Option<String>, Error> {
    if attr.name() != "amino_name" {
        return Ok(None);
    }
    match *attr {
        Meta::NameValue(ref meta_name_value) => match meta_name_value.lit {
            Lit::Str(ref lit) => Ok(Some(lit.value())),
            _ => bail!("invalid tag attribute: {:?}", attr),
        },
        _ => bail!("invalid tag attribute: {:?}", attr),
    }
}

fn tags_attr(attr: &Meta) -> Result<Option<Vec<u32>>, Error> {
    if attr.name() != "tags" {
        return Ok(None);
    }
    match *attr {
        Meta::List(ref meta_list) => {
            let mut tags = Vec::with_capacity(meta_list.nested.len());
            for item in &meta_list.nested {
                if let NestedMeta::Literal(Lit::Int(ref lit)) = *item {
                    tags.push(lit.value() as u32);
                } else {
                    bail!("invalid tag attribute: {:?}", attr);
                }
            }
            return Ok(Some(tags));
        }
        Meta::NameValue(MetaNameValue {
            lit: Lit::Str(ref lit),
            ..
        }) => lit
            .value()
            .split(',')
            .map(|s| s.trim().parse::<u32>().map_err(Error::from))
            .collect::<Result<Vec<u32>, _>>()
            .map(|tags| Some(tags)),
        _ => bail!("invalid tag attribute: {:?}", attr),
    }
}
