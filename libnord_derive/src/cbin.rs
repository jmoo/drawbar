use darling::ast::NestedMeta;
use darling::{FromField, FromMeta};
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use syn::{DeriveInput, Field, FieldsNamed, Visibility};
use syn::{parse_quote, Expr};


#[derive(Debug, FromMeta)]
struct Args {
    #[darling(default)]
    format: Option<String>,

    #[darling(default)]
    bank_count: u16,

    #[darling(default)]
    slot_count: u16,
}

#[derive(Debug, FromField, Default)]
#[darling(attributes(cbin))]
struct AttrArgs {
    #[darling(default)]
    bits: usize,

    #[darling(default)]
    bytes: usize,

    #[darling(map = Some)]
    from: Option<Expr>,

    #[darling(map = Some)]
    try_from: Option<Expr>,

    #[darling(default)]
    temp: bool,

    #[darling(default)]
    ignore: bool,
}

#[derive(Debug)]
struct Spec {
    field: Option<TokenStream>,
    map_read: Option<TokenStream>,
    bytes: usize,
    bits: usize,
}

impl Spec {
    pub fn from_field(field: &Field) -> Result<Self, syn::Error> {
        let args = AttrArgs::from_field(field)?;

        if args.ignore {
            let field = field.clone();
            return Ok(Spec {
                bytes: 0,
                bits: 0,
                map_read: None,
                field: Some(quote! {
                    #field
                })
            });
        }

        if args.from.is_some() && args.try_from.is_some() {
            return Err(syn::Error::new(
                Span::call_site(),
                "Cannot use both `from` and `try_from` attributes",
            ));
        }

        let map_read = if let Some(try_from) = args.try_from {
            quote! {
                (#try_from)(_out)?
            }
        } else if let Some(from) = args.from {
            quote! {
                (#from)(_out)
            }
        } else {
            quote! {
                _out
            }
        };

        let name = field.ident.clone().unwrap();

        let map_read = if !args.temp {
            quote! {
                instance.#name = #map_read;
            }
        } else {
            quote! {
                let mut #name = #map_read;
            }
        };

        let field = if args.temp {
            None
        } else {
            let vis = field.vis.clone();
            let ty = field.ty.clone();
            let attrs: Vec<_> =  field.attrs.iter().filter_map(|attr| {
                if attr.path().is_ident("cbin") {
                    None
                } else {
                    Some(attr.clone())
                }
            }).collect();

            Some(quote! {
                #(#attrs)*
                #vis #name: #ty
            })
        };

        Ok(Spec {
            map_read: Some(map_read),
            field,
            bits: args.bits,
            bytes: args.bytes
        })
    }
}

pub struct Generator {
    input: DeriveInput,
    name: Ident,
    vis: Visibility,
    schema: Vec<Spec>,
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

        let fields = match contents.fields {
            syn::Fields::Named(FieldsNamed { named, .. }) => named,
            _ => {
                return Err(syn::Error::new(
                    Span::call_site(),
                    "Only named fields are supported",
                ))
            }
        };

        let (specs, errors): (Vec<_>, Vec<_>) = fields
            .clone()
            .iter()
            .map(Spec::from_field)
            .partition(Result::is_ok);

        let mut specs: Vec<_> = specs.into_iter().map(Result::unwrap).collect();
        let errors: Vec<_> = errors.into_iter().map(Result::unwrap_err).collect();

        if errors.len() > 0 {
            return Err(errors[0].clone());
        }

        let mut schema: Vec<Spec> = vec![
            Spec {
                field: None, bytes: 4, bits: 0, map_read: Some(parse_quote! { 
                    let magic_str = String::from_utf8_lossy(&_out).to_string();
                    assert_eq!(magic_str, "CBIN");
                })
            },

            Spec {
                field: None, bytes: 4, bits: 0, map_read: Some(parse_quote! { 
                    let file_version = u32::from_be_bytes(_out);
                })
            },
        ];

        if let Some(format) = args.format {
            schema.append(&mut vec![
                Spec {
                    field: None, bytes: 4, bits: 0, map_read: Some(parse_quote! { 
                        let format_str = String::from_utf8_lossy(&_out).to_string();
                        assert_eq!(format_str, #format);
                    })
                },
            ]);

            if args.bank_count + args.slot_count > 0 {
                let bank_count = args.bank_count;
                let slot_count = args.slot_count;

                if bank_count == 0 || slot_count == 0 {
                    return Err(syn::Error::new(
                        Span::call_site(),
                        "Both `bank_count` and `slot_count` must be specified if one is specified",
                    ));
                }

                schema.append(&mut vec![
                    Spec {
                        bytes: 4, 
                        bits: 0, 
                        field: Some(quote! {
                            pub location: ::libnord::types::RangedU16Pair<#bank_count, #slot_count>
                        }), 
                        map_read: Some(parse_quote! {
                            let bank = u16::from_le_bytes([_out[0], _out[1]]);
                            let slot = u16::from_le_bytes([_out[2], _out[3]]);
                            instance.location = (bank, slot).try_into().unwrap();
                        })
                    }
                ]);

                schema.append(&mut vec![
                    Spec {
                        field: None, bytes: 4, bits: 0, map_read: Some(parse_quote! {
                            let header_trailer = u32::from_be_bytes(_out);
                            assert_eq!(header_trailer, 0xffffffff);
                        })
                    }
                ]);
            }
        }

        schema.append(&mut specs);

        Ok(Self {
            input: struct_input,
            name: struct_name.clone(),
            vis: struct_vis.clone(),
            schema
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
        
        let fields = self.schema.iter().filter_map(|spec| {
            spec.field.clone()
        }).collect::<Vec<_>>();

        quote! {
            #(#attrs)*
            #vis struct #name {
                #(#fields),*
            }
        }
    }

    fn expand_reader(&self) -> TokenStream {
        let name = &self.name;

        let mut total_bytes = 0;
        let mut total_bits = 0;

        let readers: Vec<_> = self.schema.iter().map(|schema| {
            if let Some(map_expr) = schema.map_read.clone() {
                let schema_bytes = schema.bytes;
                let schema_bits = schema.bits;

                total_bytes += schema_bytes;
                total_bits += schema_bits;

                let bytes = schema.bytes + (if schema.bits > 0 { (schema.bits / 8) + 1 } else { 0 });
                let bits = schema.bits % 8;

                if bytes > 0 {
                    return quote! {
                        let mut _out = [0u8; #bytes];
                        reader.read_bits_exact(&mut _out, #bits)?;
                        #map_expr
                    }
                } else {
                    return quote! {
                        println!(#schema_bits, #schema_bytes)
                        let mut _out: Vec<u8> = Vec::new();
                        reader.read_to_end(&mut _out)?;
                        #map_expr
                    }
                }
            } else {
                quote! {
                    // Ignored field (no map_read)
                }
            } 
        }).collect();

        if (total_bits % 8) > 0 {
            panic!("Total bits is not a multiple of 8");
        }

        total_bytes += total_bits % 8;

        quote! {
            impl ::libnord::cbin::FromReader<Self> for #name {
                fn from_reader(reader: &mut (impl std::io::Read)) -> Result<Self, std::io::Error> {
                    let mut instance = Self::default();
                    let mut reader = ::libnord::cbin::BitReader::new(reader);
                    // let mut crc32 = ::libnord::crc::CrcReader::new(0x2c, 0x2c - #total_bytes);
                    let size = #total_bytes;

                    #(#readers)*

                    Ok(instance)
                }
            }
        }
    }
}
