use darling::ast::NestedMeta;
use darling::FromMeta;
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use syn::parse_quote;
use syn::{DeriveInput, Field, FieldsNamed, Visibility};

use crate::spec::{Spec, SpecArgs, SpecField};

#[derive(Debug, FromMeta)]
struct Args {
    #[darling(default)]
    format: Option<String>,

    #[darling(default)]
    bank_count: u16,

    #[darling(default)]
    slot_count: u16,

    #[darling(default)]
    fragment: bool,
}

pub struct Generator {
    input: DeriveInput,
    name: Ident,
    vis: Visibility,
    spec: Spec,
}

impl Generator {
    pub fn new(args: TokenStream, input: TokenStream) -> Result<Self, syn::Error> {
        let struct_input = syn::parse::<syn::DeriveInput>(input.into())?;
        let struct_name = &struct_input.ident.clone();
        let struct_vis = &struct_input.vis.clone();

        let args = NestedMeta::parse_meta_list(args.into())?;
        let args = Args::from_list(&args)?;

        let contents = match struct_input.clone().data {
            syn::Data::Struct(s) => s,
            _ => {
                return Err(syn::Error::new(
                    Span::call_site(),
                    "Only structs are supported",
                ))
            }
        };

        let fields: Vec<Field> = match contents.fields {
            syn::Fields::Named(FieldsNamed { named, .. }) => {
                named.iter().map(|f| f.clone()).collect()
            }
            _ => {
                return Err(syn::Error::new(
                    Span::call_site(),
                    "Only named fields are supported",
                ))
            }
        };

        let mut spec = Spec::new(&fields)?;

        if !args.fragment {
            spec.append(vec![
                // 0x00 - CBIN magic string
                SpecField::new(SpecArgs {
                    name: Some(parse_quote! { _cbin_magic_str }),
                    from: Some(parse_quote! {
                        |x: [u8; 4]| {
                            let mapped = String::from_utf8_lossy(&x).to_string();
                            assert_eq!(mapped, "CBIN"); mapped
                        }
                    }),
                    mapped_type: Some(parse_quote! { String }),
                    seek: Some(0x00),
                    temp: true,
                    bytes: 4,
                    ..Default::default()
                })?
                .unwrap(),
                // 0x04 - CBIN file version
                SpecField::new(SpecArgs {
                    name: Some(parse_quote! { _cbin_version }),
                    mapped_type: Some(parse_quote! { u32 }),
                    seek: Some(0x04),
                    temp: true,
                    ..Default::default()
                })?
                .unwrap(),
                // 0x08 - CBIN file format
                SpecField::new(SpecArgs {
                    name: Some(parse_quote! { _cbin_format }),
                    from: Some(if let Some(format) = args.format {
                        parse_quote! {
                            |x: [u8; 4]| {
                                let mapped = String::from_utf8_lossy(&x).to_string();
                                assert_eq!(mapped, #format); mapped
                            }
                        }
                    } else {
                        parse_quote! { |x: [u8; 4]| String::from_utf8_lossy(&x).to_string() }
                    }),
                    mapped_type: Some(parse_quote! { String }),
                    seek: Some(0x08),
                    temp: true,
                    bytes: 4,
                    ..Default::default()
                })?
                .unwrap(),
                // 0x10 - CBIN header trailer
                SpecField::new(SpecArgs {
                    name: Some(parse_quote! { _cbin_trailer }),
                    mapped_type: Some(parse_quote! { u32 }),
                    from: Some(parse_quote! {
                        |x: [u8; 4]| {
                            let mapped = u32::from_be_bytes(x);
                            assert_eq!(mapped, 0xffffffff); mapped
                        }
                    }),
                    bytes: 4,
                    seek: Some(0x10),
                    temp: true,
                    ..Default::default()
                })?
                .unwrap(),
            ]);

            // 0x0C - CBIN entity bank location
            if args.bank_count + args.slot_count > 0 {
                let bank_count = args.bank_count;
                let slot_count = args.slot_count;

                if bank_count == 0 || slot_count == 0 {
                    return Err(syn::Error::new(
                        Span::call_site(),
                        "Both `bank_count` and `slot_count` must be specified if one is specified",
                    ));
                }

                spec.append(vec! [
                    SpecField::new(SpecArgs {
                        name: Some(parse_quote! { location}),
                        mapped_type: Some(parse_quote! { ::libnord::types::RangedU16Pair<#bank_count, #slot_count> }),
                        from: Some(parse_quote! { |x: [u8; 4] | (u16::from_le_bytes([x[0], x[1]]), u16::from_le_bytes([x[2], x[3]]))
                            .try_into()
                            .unwrap()
                        }),
                        seek: Some(0x0C),
                        visibility: Some(parse_quote! { pub }),
                        bytes: 4,
                        ..Default::default()
                    })?.unwrap(),
                ]);
            };
        }

        Ok(Self {
            input: struct_input,
            name: struct_name.clone(),
            vis: struct_vis.clone(),
            spec,
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
        let attrs = &self.input.attrs;

        let fields = self
            .spec
            .iter()
            .filter_map(|spec| spec.define())
            .collect::<Vec<TokenStream>>();

        quote! {
            #(#attrs)*
            #vis struct #name {
                #(#fields),*
            }
        }
    }

    fn expand_reader(&self) -> TokenStream {
        let name = &self.name;
        let size = self.spec.size();
        let bytes = self.spec.bytes();
        let bits = self.spec.bits();

        let buffer = quote! { _buffer };
        let instance = quote! { instance };

        let readers = self
            .spec
            .iter()
            .map(|spec| {
                spec.assign(
                    spec.map_read(spec.read(buffer.clone())),
                    Some(instance.clone()),
                )
            })
            .collect::<Vec<TokenStream>>();

        quote! {
            impl ::libnord::cbin::FromReader<Self> for #name {
                fn from_reader(reader: &mut (impl std::io::Read)) -> Result<Self, std::io::Error> {
                    let mut _buffer = [0u8; #size];

                    reader.read_exact(&mut _buffer)?;

                    <Self as ::libnord::cbin::FromBytes<Self>>::from_bytes(&_buffer)
                }
            }

            impl ::libnord::cbin::FromBytes<Self> for #name {
                const BYTES: usize = #bytes;
                const BITS: usize = #bits;

                fn from_bytes(_buffer: &[u8]) -> Result<Self, std::io::Error> {
                    let mut instance = Self::default();

                    #(#readers);*;

                    Ok(instance)
                }
            }
        }
    }
}
