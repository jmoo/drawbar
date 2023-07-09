use darling::ast::NestedMeta;
use darling::{FromField, FromMeta};
use proc_macro2::{Ident, Span, TokenStream};
use quote::{quote, ToTokens};
use syn::token::Type;
use syn::{parse_quote, Expr, TypeArray};
use syn::{DeriveInput, Field, FieldsNamed, Visibility};

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
        let mut args = AttrArgs::from_field(field)?;

        if args.ignore {
            let field = field.clone();
            return Ok(Spec {
                bytes: 0,
                bits: 0,
                map_read: None,
                field: Some(quote! {
                    #field
                }),
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
                (#try_from)(&_out)?
            }
        } else if let Some(from) = args.from {
            quote! {
                (#from)(&_out)
            }
        } else {
            match field.ty.clone() {
                syn::Type::Array(arr) => {
                    let size: usize = syn::LitInt::new(&arr.len.to_token_stream().to_string(), Span::call_site())
                        .base10_parse()
                        .unwrap();

                    if args.bytes == 0 && args.bits == 0 {
                        args.bytes = size;
                    } else {
                        let buffer_size = args.bytes + (if args.bits > 0 { (args.bits / 8) + 1 } else { 0 });

                        if buffer_size != size {
                            panic!("Unable to map [u8; {}] to [u8; {}] for field {}", size, buffer_size, field.ident.to_token_stream().to_string())
                        }
                    }

                    quote! {
                        _out.try_into().unwrap()
                    }
                },
                // syn::Type::BareFn(_) => todo!(),
                // syn::Type::Group(_) => todo!(),
                // syn::Type::ImplTrait(_) => todo!(),
                // syn::Type::Infer(_) => todo!(),
                // syn::Type::Macro(_) => todo!(),
                // syn::Type::Never(_) => todo!(),
                // syn::Type::Paren(_) => todo!(),
                // syn::Type::Path(_) => todo!(),
                // syn::Type::Ptr(_) => todo!(),
                // syn::Type::Reference(_) => todo!(),
                // syn::Type::Slice(_) => todo!(),
                // syn::Type::TraitObject(_) => todo!(),
                // syn::Type::Tuple(_) => todo!(),
                syn::Type::Path(expr) => {
                    let ty = expr.to_token_stream().to_string();

                    match ty.as_str() {
                        "u8" => {
                            if args.bytes == 0 && args.bits == 0 {
                                args.bytes = 1;
                            } else {
                                let buffer_size = args.bytes + (if args.bits > 0 { (args.bits / 8) + 1 } else { 0 });

                                if buffer_size != 1 {
                                    panic!("Unable to map [u8; {}] to u8 for field {}", buffer_size, field.ident.to_token_stream().to_string())
                                }
                            }

                            quote! {
                                u8::from_be_bytes([_out[0]]);
                            }
                        },
                        _ => panic!("No default mapper for [u8] -> {} for filed {}", ty, field.ident.to_token_stream().to_string())
                    }

                },
                _ => panic!("Unable to map [u8] to {} for field '{}'", field.ty.to_token_stream().to_string(), field.ident.to_token_stream().to_string())
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
            let attrs: Vec<_> = field
                .attrs
                .iter()
                .filter_map(|attr| {
                    if attr.path().is_ident("cbin") {
                        None
                    } else {
                        Some(attr.clone())
                    }
                })
                .collect();

            Some(quote! {
                #(#attrs)*
                #vis #name: #ty
            })
        };

        Ok(Spec {
            map_read: Some(map_read),
            field,
            bits: args.bits,
            bytes: args.bytes,
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
                field: None,
                bytes: 4,
                bits: 0,
                map_read: Some(parse_quote! {
                    let magic_str = String::from_utf8_lossy(&_out).to_string();
                    assert_eq!(magic_str, "CBIN");
                }),
            },
            Spec {
                field: None,
                bytes: 4,
                bits: 0,
                map_read: Some(parse_quote! {
                    let file_version = u32::from_be_bytes(_out.try_into().unwrap());
                }),
            },
        ];

        if let Some(format) = args.format {
            schema.append(&mut vec![Spec {
                field: None,
                bytes: 4,
                bits: 0,
                map_read: Some(parse_quote! {
                    let format_str = String::from_utf8_lossy(&_out).to_string();
                    assert_eq!(format_str, #format);
                }),
            }]);

            if args.bank_count + args.slot_count > 0 {
                let bank_count = args.bank_count;
                let slot_count = args.slot_count;

                if bank_count == 0 || slot_count == 0 {
                    return Err(syn::Error::new(
                        Span::call_site(),
                        "Both `bank_count` and `slot_count` must be specified if one is specified",
                    ));
                }

                schema.append(&mut vec![Spec {
                    bytes: 4,
                    bits: 0,
                    field: Some(quote! {
                        pub location: ::libnord::types::RangedU16Pair<#bank_count, #slot_count>
                    }),
                    map_read: Some(parse_quote! {
                        let bank = u16::from_le_bytes([_out[0], _out[1]]);
                        let slot = u16::from_le_bytes([_out[2], _out[3]]);
                        instance.location = (bank, slot).try_into().unwrap();
                    }),
                }]);

                schema.append(&mut vec![Spec {
                    field: None,
                    bytes: 4,
                    bits: 0,
                    map_read: Some(parse_quote! {
                        let header_trailer = u32::from_be_bytes(_out.try_into().unwrap());
                        assert_eq!(header_trailer, 0xffffffff);
                    }),
                }]);
            }
        }

        schema.append(&mut specs);

        Ok(Self {
            input: struct_input,
            name: struct_name.clone(),
            vis: struct_vis.clone(),
            schema,
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
            .schema
            .iter()
            .filter_map(|spec| spec.field.clone())
            .collect::<Vec<_>>();

        quote! {
            #(#attrs)*
            #vis struct #name {
                #(#fields),*
            }
        }
    }

    fn expand_reader(&self) -> TokenStream {
        let name = &self.name;

        let mut cursor: usize = 0;
        let mut offset: usize = 0;

        let mut readers: Vec<TokenStream> = vec![];

        for schema in self.schema.iter() {
            assert!(offset < 8);

            if schema.map_read.is_none() {
                continue;
            }

            let map_expr = schema.map_read.clone().unwrap();
            let buffer_size = schema.bytes + (if schema.bits > 0 { (schema.bits / 8) + 1 } else { 0 });
            let start = cursor;
            let end = cursor + buffer_size;

            if offset > 0 || (schema.bits % 8 > 0) {
                readers.push(quote! {
                    let mut _out = [0u8; #buffer_size];
                });

                for i in 0..buffer_size {
                    // total bits needed
                    let need = if i > 0 || schema.bits % 8 == 0 { 8 } else { schema.bits % 8 };

                    // bits to skip from the left
                    let skip = offset + 0;

                    // bits to keep in the current byte
                    let keep = 8 - (8 - need).max(offset);

                    // bits needed from the next byte
                    let replace = need - keep;

                    if replace > 0 {
                        readers.push(quote! {
                            _out[#i] = ((_buffer[#cursor] << #skip) >> (8 - #need)) | (_buffer[#cursor + 1] >> (8 - #replace));
                        });
                    } else {
                        readers.push(quote! {
                            _out[#i] = (_buffer[#cursor] << #skip) >> (8 - #need);
                        });
                    }

                    if replace > 0 || (offset + need) == 8 {
                        cursor += 1;
                        offset = replace;
                    } else {
                        offset = (offset + need) % 8;
                    }   
                }

            } else {
                readers.push(quote! {
                    let _out = &_buffer[#start..#end];
                });

                cursor = end;
            }

            readers.push(quote! {
                #map_expr
            });

        }

        quote! {
            impl ::libnord::cbin::FromReader<Self> for #name {
                fn from_reader(reader: &mut (impl std::io::Read)) -> Result<Self, std::io::Error> {
                    let mut _buffer = [0u8; #cursor];

                    reader.read_exact(&mut _buffer)?;

                    <Self as ::libnord::cbin::FromBytes<Self>>::from_bytes(&_buffer)
                }
            }

            impl ::libnord::cbin::FromBytes<Self> for #name {
                fn from_bytes(_buffer: &[u8]) -> Result<Self, std::io::Error> {
                    let mut instance = Self::default();

                    #(#readers)*

                    Ok(instance)
                }
            }
        }
    }
}
