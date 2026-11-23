//! Generates game value schemas and codecs from Rust declarations.

use proc_macro::TokenStream;
use proc_macro2::{Ident, Span, TokenStream as Tokens};
use quote::{format_ident, quote};
use syn::{parse_macro_input, parse_quote, Attribute, Data, DeriveInput, Fields, Meta, Type};

/// Defines a game value’s schema and encoder, and a decoder when its fields support reading.
/// Declaration order determines wire positions. Serde names describe the public representation.
#[proc_macro_derive(GameValue, attributes(serde))]
pub fn game_value(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand(input.clone())
        .and_then(|writer| {
            let decoder = expand_decoder(input)?;
            Ok(quote!(#writer #decoder))
        })
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Serialization attributes that affect the named representation supported by this derive.
#[derive(Default)]
struct Names {
    rename: Option<String>,
    rename_all: Option<String>,
    tag: Option<String>,
    content: Option<String>,
    omit_none: bool,
}

/// Rejects unsupported shape-changing attributes instead of advertising an inaccurate schema.
fn names(attrs: &[Attribute]) -> syn::Result<Names> {
    let mut names = Names::default();
    for attr in attrs.iter().filter(|a| a.path().is_ident("serde")) {
        for item in attr.parse_args_with(
            syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated,
        )? {
            let path = item.path();
            if path.is_ident("deny_unknown_fields")
                || path.is_ident("default")
                || path.is_ident("borrow")
            {
                continue;
            }
            let value = match &item {
                Meta::NameValue(nv) => match &nv.value {
                    syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(v),
                        ..
                    }) => v.value(),
                    _ => return Err(syn::Error::new_spanned(item, "expected a string literal")),
                },
                _ => {
                    return Err(syn::Error::new_spanned(
                        item,
                        "unsupported Serde attribute in component schema",
                    ))
                }
            };
            if path.is_ident("rename") {
                names.rename = Some(value);
            } else if path.is_ident("rename_all") {
                names.rename_all = Some(value);
            } else if path.is_ident("tag") {
                names.tag = Some(value);
            } else if path.is_ident("content") {
                names.content = Some(value);
            } else if path.is_ident("skip_serializing_if") && value == "Option::is_none" {
                names.omit_none = true;
            } else {
                return Err(syn::Error::new_spanned(
                    item,
                    "unsupported Serde attribute in component schema",
                ));
            }
        }
    }
    Ok(names)
}

/// Applies Serde's field or variant case convention, including initialisms in variant names.
fn wire_name(id: &Ident, attrs: &Names, rule: Option<&str>, variant: bool) -> syn::Result<String> {
    if let Some(name) = &attrs.rename {
        return Ok(name.clone());
    }
    let ident = id.to_string();
    let name = ident.trim_start_matches("r#");
    let Some(rule) = rule else {
        return Ok(name.to_owned());
    };
    let snake = if variant {
        let mut s = String::new();
        for (i, c) in name.char_indices() {
            if i > 0 && c.is_uppercase() {
                s.push('_');
            }
            s.push(c.to_ascii_lowercase());
        }
        s
    } else {
        name.to_owned()
    };
    let pascal = if variant {
        name.to_owned()
    } else {
        let mut s = String::new();
        for word in name.split('_') {
            let mut chars = word.chars();
            if let Some(c) = chars.next() {
                s.push(c.to_ascii_uppercase());
                s.extend(chars);
            }
        }
        s
    };
    Ok(match rule {
        "lowercase" if variant => name.to_ascii_lowercase(),
        "lowercase" => name.to_owned(),
        "UPPERCASE" => name.to_ascii_uppercase(),
        "PascalCase" => pascal,
        "camelCase" => {
            let mut chars = pascal.chars();
            chars
                .next()
                .map(|c| c.to_ascii_lowercase().to_string() + chars.as_str())
                .unwrap_or_default()
        }
        "snake_case" => snake,
        "SCREAMING_SNAKE_CASE" => snake.to_ascii_uppercase(),
        "kebab-case" => snake.replace('_', "-"),
        "SCREAMING-KEBAB-CASE" => snake.to_ascii_uppercase().replace('_', "-"),
        _ => return Err(syn::Error::new_spanned(id, "unknown Serde rename_all rule")),
    })
}

/// Collects field schemas and the corresponding borrows used by the generated writer.
struct Members {
    named: Vec<Tokens>,
    types: Vec<Type>,
    bindings: Vec<Ident>,
    accesses: Vec<Tokens>,
}

impl Members {
    /// Checks public key uniqueness and collects each field exactly once in declaration order.
    fn new(fields: &Fields, rename: Option<&str>) -> syn::Result<Self> {
        let mut out = Self {
            named: vec![],
            types: vec![],
            bindings: vec![],
            accesses: vec![],
        };
        let mut used = std::collections::HashSet::new();
        for (i, field) in fields.iter().enumerate() {
            let attrs = names(&field.attrs)?;
            let ty = &field.ty;
            let bind = format_ident!("__field_{i}");
            if let Some(id) = &field.ident {
                let name = wire_name(id, &attrs, rename, false)?;
                if !used.insert(name.clone()) {
                    return Err(syn::Error::new_spanned(
                        field,
                        "duplicate component field name",
                    ));
                }
                let omit = attrs.omit_none;
                out.named.push(quote!(::parlando_game_sdk::Field{name:#name.into(),schema:<#ty as ::parlando_game_sdk::GameValue>::schema(),omit_none:#omit}));
                out.accesses.push(quote!(&self.#id));
            } else {
                let index = syn::Index::from(i);
                out.accesses.push(quote!(&self.#index));
            }
            out.types.push(ty.clone());
            out.bindings.push(bind);
        }
        Ok(out)
    }
}

/// Writes one array, with a numeric discriminator first for payload-bearing enums.
fn tuple(values: &[Tokens], tag: Option<u32>) -> Tokens {
    let len = values.len() + usize::from(tag.is_some());
    let tag = tag.map(|n| quote!(::parlando_game_sdk::__serde::ser::SerializeTuple::serialize_element(&mut seq,&#n)?;));
    quote!({
        let mut seq=::parlando_game_sdk::__serde::Serializer::serialize_tuple(serializer,#len)?;
        #tag
        #(::parlando_game_sdk::__serde::ser::SerializeTuple::serialize_element(&mut seq,&::parlando_game_sdk::Compact(#values))?;)*
        ::parlando_game_sdk::__serde::ser::SerializeTuple::end(seq)
    })
}

/// Builds a matching declaration and writer, rejecting ambiguous representations.
fn expand(input: DeriveInput) -> syn::Result<Tokens> {
    let attrs = names(&input.attrs)?;
    let (schema, writer) = match &input.data {
        Data::Struct(data) => {
            let m = Members::new(&data.fields, attrs.rename_all.as_deref())?;
            let fields = &m.named;
            let types = &m.types;
            match &data.fields {
                Fields::Unit => (
                    quote!(::parlando_game_sdk::Schema::Null),
                    quote!(::parlando_game_sdk::__serde::Serializer::serialize_unit(
                        serializer
                    )),
                ),
                Fields::Unnamed(f) if f.unnamed.len() == 1 => {
                    let ty = &types[0];
                    (
                        quote!(<#ty as ::parlando_game_sdk::GameValue>::schema()),
                        quote!(::parlando_game_sdk::GameValue::encode_compact(
                            &self.0, serializer
                        )),
                    )
                }
                Fields::Unnamed(_) => (
                    quote!(::parlando_game_sdk::Schema::Tuple(
                        vec![#(<#types as ::parlando_game_sdk::GameValue>::schema()),*]
                    )),
                    tuple(&m.accesses, None),
                ),
                Fields::Named(_) => (
                    quote!(::parlando_game_sdk::Schema::Struct(vec![#(#fields),*])),
                    tuple(&m.accesses, None),
                ),
            }
        }
        Data::Enum(data) => {
            let unit_only = data
                .variants
                .iter()
                .all(|v| matches!(v.fields, Fields::Unit));
            let repr = match (&attrs.tag, &attrs.content) {
                (None, None) => quote!(::parlando_game_sdk::EnumRepresentation::External),
                (Some(tag), None) => {
                    quote!(::parlando_game_sdk::EnumRepresentation::Internal{tag:#tag.into()})
                }
                (Some(tag), Some(content)) => {
                    quote!(::parlando_game_sdk::EnumRepresentation::Adjacent{tag:#tag.into(),content:#content.into()})
                }
                _ => {
                    return Err(syn::Error::new_spanned(
                        &input,
                        "enum content requires a tag",
                    ))
                }
            };
            let mut variants = vec![];
            let mut arms = vec![];
            let mut used = std::collections::HashSet::new();
            for (i, v) in data.variants.iter().enumerate() {
                let a = names(&v.attrs)?;
                let id = &v.ident;
                let name = wire_name(id, &a, attrs.rename_all.as_deref(), true)?;
                if !used.insert(name.clone()) {
                    return Err(syn::Error::new_spanned(
                        v,
                        "duplicate component variant name",
                    ));
                }
                let m = Members::new(&v.fields, a.rename_all.as_deref())?;
                let bindings = &m.bindings;
                let fields = &m.named;
                let types = &m.types;
                let (pattern, payload) = match &v.fields {
                    Fields::Unit => (quote!(), quote!(::parlando_game_sdk::VariantPayload::Unit)),
                    Fields::Named(fs) => {
                        let ids = fs.named.iter().map(|f| f.ident.as_ref().unwrap());
                        (
                            quote!({#(#ids:#bindings),*}),
                            quote!(::parlando_game_sdk::VariantPayload::Struct(
                                vec![#(#fields),*]
                            )),
                        )
                    }
                    Fields::Unnamed(_) if bindings.len() == 1 => {
                        let ty = &types[0];
                        (
                            quote!((#(#bindings),*)),
                            quote!(::parlando_game_sdk::VariantPayload::Newtype(Box::new(<#ty as ::parlando_game_sdk::GameValue>::schema()))),
                        )
                    }
                    Fields::Unnamed(_) => (
                        quote!((#(#bindings),*)),
                        quote!(::parlando_game_sdk::VariantPayload::Tuple(
                            vec![#(<#types as ::parlando_game_sdk::GameValue>::schema()),*]
                        )),
                    ),
                };
                variants
                    .push(quote!(::parlando_game_sdk::Variant{name:#name.into(),payload:#payload}));
                let tag = i as u32;
                let write = if unit_only {
                    quote!(::parlando_game_sdk::__serde::Serializer::serialize_u32(serializer,#tag))
                } else {
                    tuple(
                        &bindings.iter().map(|b| quote!(#b)).collect::<Vec<_>>(),
                        Some(tag),
                    )
                };
                arms.push(quote!(Self::#id #pattern => #write));
            }
            (
                quote!(::parlando_game_sdk::Schema::Enum{representation:#repr,variants:vec![#(#variants),*]}),
                quote!(match self {#(#arms),*}),
            )
        }
        Data::Union(_) => {
            return Err(syn::Error::new(
                Span::call_site(),
                "unions cannot define a component value",
            ))
        }
    };
    let mut generics = input.generics;
    for param in generics.type_params_mut() {
        param
            .bounds
            .push(parse_quote!(::parlando_game_sdk::GameValue));
    }
    let (imp, ty, clause) = generics.split_for_impl();
    let id = input.ident;
    Ok(quote! {
        impl #imp ::parlando_game_sdk::GameValue for #id #ty #clause {
            fn schema()->::parlando_game_sdk::Schema{#schema}
            fn encode_compact<__S: ::parlando_game_sdk::__serde::Serializer>(&self,serializer:__S)->::std::result::Result<__S::Ok,__S::Error>{#writer}
        }
    })
}

/// Builds a struct or variant from its ordered field slots without using Serde names.
fn read_fields(constructor: Tokens, fields: &Fields) -> Tokens {
    match fields {
        Fields::Unit => constructor,
        Fields::Unnamed(fields) => {
            let slots = fields.unnamed.iter().map(|_| quote!(reader.read()?));
            quote!(#constructor(#(#slots),*))
        }
        Fields::Named(fields) => {
            let ids = fields.named.iter().map(|field| &field.ident);
            quote!(#constructor { #(#ids: reader.read()?),* })
        }
    }
}

/// Generates positional construction for transparent newtypes, fixed-slot
/// records, and enums whose payload length is determined by their ordinal.
fn expand_decoder(input: DeriveInput) -> syn::Result<Tokens> {
    let body = match &input.data {
        Data::Struct(data) => {
            let value = read_fields(quote!(Self), &data.fields);
            let len = data.fields.len();
            match &data.fields {
                Fields::Unit => quote! {
                    reader.read::<()>()?;
                    Ok(#value)
                },
                Fields::Unnamed(_) if len == 1 => quote!(Ok(#value)),
                _ => quote! {
                    ::parlando_game_sdk::CompactReader::check_slots(reader.array()?, #len)?;
                    reader.nested(|reader| Ok(#value))
                },
            }
        }
        Data::Enum(data) => {
            let unit_only = data
                .variants
                .iter()
                .all(|variant| matches!(variant.fields, Fields::Unit));
            let arms = data.variants.iter().enumerate().map(|(i, variant)| {
                let ordinal = i as u32;
                let id = &variant.ident;
                let value = read_fields(quote!(Self::#id), &variant.fields);
                if unit_only {
                    return quote!(#ordinal => Ok(#value));
                }
                let len = variant.fields.len() + 1;
                let read = if variant.fields.is_empty() {
                    quote!(Ok(#value))
                } else {
                    quote!(reader.nested(|reader| Ok(#value)))
                };
                quote! {
                    #ordinal => {
                        ::parlando_game_sdk::CompactReader::check_slots(__slots, #len)?;
                        #read
                    }
                }
            });
            let header = (!unit_only).then(|| quote!(let __slots = reader.array()?;));
            quote! {
                #header
                match reader.ordinal()? {
                    #(#arms,)*
                    _ => ::parlando_game_sdk::CompactReader::invalid_ordinal(),
                }
            }
        }
        Data::Union(_) => {
            return Err(syn::Error::new(
                Span::call_site(),
                "unions cannot decode a component value",
            ));
        }
    };
    let mut generics = input.generics.clone();
    let decode_lifetime = (0..)
        .map(|n| syn::Lifetime::new(&format!("'__parlando_de_{n}"), Span::call_site()))
        .find(|name| {
            input
                .generics
                .lifetimes()
                .all(|param| param.lifetime != *name)
        })
        .expect("a fresh lifetime name");
    generics.params.insert(0, parse_quote!(#decode_lifetime));
    let fields: Vec<_> = match &input.data {
        Data::Struct(data) => data.fields.iter().collect(),
        Data::Enum(data) => data
            .variants
            .iter()
            .flat_map(|variant| &variant.fields)
            .collect(),
        Data::Union(_) => unreachable!(),
    };
    let id = &input.ident;
    let (_, ty, _) = input.generics.split_for_impl();
    let clause = generics.make_where_clause();
    clause
        .predicates
        .push(parse_quote!(#id #ty: ::parlando_game_sdk::GameValue));
    for field in fields {
        let ty = &field.ty;
        clause
            .predicates
            .push(parse_quote!(#ty: ::parlando_game_sdk::DecodeGameValue<#decode_lifetime>));
    }
    let (imp, _, clause) = generics.split_for_impl();
    Ok(quote! {
        impl #imp ::parlando_game_sdk::DecodeGameValue<#decode_lifetime> for #id #ty #clause {
            fn read_compact(reader: &mut ::parlando_game_sdk::CompactReader<#decode_lifetime>) -> ::parlando_game_sdk::CompactDecodeResult<Self> {
                #body
            }
        }
    })
}
