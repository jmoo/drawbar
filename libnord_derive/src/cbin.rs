use std::u8::MAX;

use darling::ast::NestedMeta;
use darling::{FromField, FromMeta, util::parse_expr};
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use syn::{DeriveInput, Field, FieldsNamed, Visibility};
use syn::{parse_quote, Expr};


#[derive(Debug, FromMeta)]
struct CBinArgs {}

#[derive(Debug, FromField)]
#[darling(attributes(cbin))]
struct CBinAttrArgs {
    #[darling(default)]
    bits: usize,

    #[darling(default)]
    bytes: usize,

    #[darling(map = Some)]
    from: Option<Expr>,

    #[darling(map = Some)]
    try_from: Option<Expr>,
}

pub struct CBinGenerator {
    definition: DeriveInput,
    name: Ident,
    vis: Visibility,
    args: CBinArgs,
    schema: Vec<CBinAttrArgs>,
    fields: Vec<Field>,
}

impl CBinGenerator {
    pub fn new(args: TokenStream, input: TokenStream) -> Result<Self, syn::Error> {
        let struct_definition = syn::parse::<syn::DeriveInput>(input.into())?;
        let struct_name = &struct_definition.ident.clone();
        let struct_vis = &struct_definition.vis.clone();

        let struct_args = NestedMeta::parse_meta_list(args.into())?;
        let struct_args = CBinArgs::from_list(&struct_args)?;

        let contents = match struct_definition.clone().data {
            syn::Data::Struct(s) => s,
            _ => {
                return Err(syn::Error::new(
                    Span::call_site(),
                    "Only structs are supported",
                ))
            }
        };

        let fields = match contents.fields {
            syn::Fields::Named(FieldsNamed { named, .. }) => named,
            _ => {
                return Err(syn::Error::new(
                    Span::call_site(),
                    "Only named fields are supported",
                ))
            }
        };

        let (schema, errors): (Vec<_>, Vec<_>) = fields
            .iter()
            .map(|field| CBinAttrArgs::from_field(field))
            .partition(Result::is_ok);

        let schema: Vec<_> = schema.into_iter().map(Result::unwrap).collect();
        let errors: Vec<_> = errors.into_iter().map(Result::unwrap_err).collect();

        let fields: Vec<_> = fields
            .clone()
            .iter_mut()
            .map(|field| {
                let filtered_attrs = field.attrs.iter_mut().filter_map(|attr| {
                    if attr.path().is_ident("cbin") {
                        None
                    } else {
                        Some(attr.clone())
                    }
                });

                field.attrs = filtered_attrs.collect();
                field
            })
            .map(|f| f.clone())
            .collect();

        if errors.len() > 0 {
            return Err(syn::Error::new(Span::call_site(), errors[0].to_string()));
        }

        Ok(Self {
            definition: struct_definition,
            name: struct_name.clone(),
            vis: struct_vis.clone(),
            args: struct_args,
            schema,
            fields,
        })
    }

    pub fn expand(&mut self) -> syn::Result<TokenStream> {
        let expanded_struct = self.expand_struct();
        let expanded_reader = self.expand_reader();

        let output = quote! {
            #expanded_struct
            #expanded_reader
        };

        Ok(output)
    }

    fn expand_struct(&self) -> TokenStream {
        let name = &self.name;
        let vis = &self.vis;
        let attrs = &self.definition.attrs;
        let fields = &self.fields;

        quote! {
            #(#attrs)*
            #vis struct #name {
                #(#fields),*
            }
        }
    }

    fn expand_reader(&self) -> TokenStream {
        let name = &self.name;
        let debug_args = format!("{:?}", self.args);
        let debug_fields = format!("{:?}", self.schema);

        let mut i = 0;
        let read_mappers = self.schema.iter().map(|schema| {
            let field = &self.fields[i];
            i = i + 1;

            let name = field.ident.clone().unwrap();
            
            // size config
            let bytes = &schema.bytes + (if schema.bits > 0 { (schema.bits / 8) + 1 } else { 0 });
            let bits = schema.bits % 8;

            // mapping overrides
            let from = &schema.from;
            let try_from = &schema.try_from;

            let read_expr = if bytes> 0 {
                quote! {
                    let mut out = [0u8; #bytes];
                    reader.read_bits_exact(&mut out, #bits)?;
                }
            } else {
                quote! {
                    let mut out: Vec<u8> = Vec::new();
                    reader.read_to_end(&mut out)?;
                }
            };

            if let Some(try_from) = try_from {
                quote! {
                    #read_expr
                    instance.#name = (#try_from)(out)?;
                }
            } else if let Some(from) = from {
                quote! {
                    #read_expr
                    instance.#name = (#from)(out);
                }
            } else {
                quote! {
                    #read_expr
                    instance.#name = out;
                }
            }
        });

        quote! {
            impl ::libnord::cbin::FromReader<Self> for #name {
                fn from_reader(reader: &mut (impl std::io::Read)) -> Result<Self, std::io::Error> {
                    let mut instance = Self::default();
                    let mut reader = ::libnord::cbin::BitReader::new(reader);

                    let args = #debug_args;
                    let fields = #debug_fields;

                    #(#read_mappers)*

                    Ok(instance)
                }
            }
        }
    }
}
