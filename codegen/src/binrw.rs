use darling::FromField;
use proc_macro2::{Ident, Span, TokenStream};
use quote::{quote, ToTokens};
use std::{collections::VecDeque};
use syn::{Attribute, Expr, Field, Visibility};

#[derive(Debug, FromField, Default)]
#[darling(attributes(cbin))]
pub struct SpecArgs {
    #[darling(default)]
    pub bits: usize,

    #[darling(default)]
    pub bytes: usize,

    #[darling(map = Some)]
    pub from: Option<Expr>,

    #[darling(map = Some)]
    pub try_from: Option<Expr>,

    #[darling(default)]
    pub temp: bool,

    #[darling(default)]
    pub ignore: bool,

    #[darling(map = Some)]
    pub seek: Option<usize>,

    #[darling(skip)]
    pub name: Option<Ident>,

    #[darling(skip)]
    pub visibility: Option<Visibility>,

    #[darling(skip)]
    pub mapped_type: Option<syn::Type>,

    #[darling(skip)]
    pub attributes: Option<Vec<Attribute>>,
}

#[derive(Debug)]
pub struct SpecField {
    pub map_read: Option<TokenStream>,
    pub map_write: Option<TokenStream>,
    pub infallible_read: bool,
    pub infallible_write: bool,
    pub name: Ident,
    pub vis: Option<Visibility>,
    pub attrs: Vec<Attribute>,
    pub ty: syn::Type,
    pub bytes: usize,
    pub bits: usize,
    pub cursor: usize,
    pub offset: usize,
    pub size: usize,
    pub index: usize,
    pub pinned: bool,
}

impl SpecField {
    pub fn new(args: SpecArgs) -> Result<Option<Self>, syn::Error> {
        let mut size = args.bytes
            + (if args.bits > 0 {
                (args.bits / 8) + 1
            } else {
                0
            });
        let mut bytes = args.bytes + (args.bits / 8);
        let bits = args.bits % 8;

        if args.ignore {
            return Ok(None);
        }

        if args.from.is_some() && args.try_from.is_some() {
            return Err(syn::Error::new(
                Span::call_site(),
                "Cannot use both `from` and `try_from` attributes",
            ));
        }

        let name = args.name.unwrap();
        let ty = args.mapped_type.unwrap();

        let mut infallible_read = true;
        let infallible_write = true;

        let map_read = if let Some(try_from) = args.try_from {
            infallible_read = false;
            Some(quote! {
                #try_from
            })
        } else if let Some(from) = args.from {
            Some(quote! {
                #from
            })
        } else {
            let ty_string = ty.to_token_stream().to_string();
            let name_string = name.to_token_stream().to_string();

            let (mapped_size, mapper) = match ty.clone() {
                syn::Type::Array(arr) => {
                    let arr_size: usize =
                        syn::LitInt::new(&arr.len.to_token_stream().to_string(), Span::call_site())
                            .base10_parse()
                            .unwrap();

                    (arr_size, None)
                }

                syn::Type::Path(expr) => {
                    let tup = match ty_string.as_str() {
                        "u8" => (1, quote! { u8::from_be_bytes }),
                        "u16" => (2, quote! { u16::from_be_bytes }),
                        "u32" => (4, quote! { u32::from_be_bytes }),
                        "u64" => (8, quote! { u64::from_be_bytes }),
                        "u128" => (16, quote! { u128::from_be_bytes }),
                        "String" => (
                            size,
                            quote! { | x: [u8; #size] | String::from_utf8_lossy(&x).to_string() },
                        ),
                        _ => {
                            infallible_read = true;
                            (
                                size,
                                quote! {
                                    | x: [u8; #size] | x.try_into().unwrap()
                                },
                            )
                        }
                    };

                    (tup.0, Some(tup.1))
                }
                _ => panic!(
                    "Unable to map [u8] to {} for field '{}'",
                    ty_string, name_string
                ),
            };

            if bytes == 0 && bits == 0 {
                bytes = mapped_size;
                size = mapped_size;
            }

            if size != mapped_size {
                panic!(
                    "Unable to map [u8; {}] to {} ({} bytes) for field '{}'",
                    size, ty_string, mapped_size, name_string
                )
            }

            if size == 0 {
                panic!(
                    "Unable to determin size of field '{}'",
                    name.to_token_stream().to_string()
                )
            }

            mapper
        };

        Ok(Some(Self {
            map_write: None,
            vis: if args.temp { None } else { args.visibility },
            attrs: args
                .attributes
                .unwrap_or(Vec::new())
                .iter()
                .filter_map(|attr| {
                    if attr.path().is_ident("cbin") {
                        None
                    } else {
                        Some(attr.clone())
                    }
                })
                .collect(),
            cursor: args.seek.unwrap_or(0),
            pinned: args.seek.is_some(),
            offset: 0,
            index: 0,
            name,
            ty,
            map_read,
            infallible_read,
            infallible_write,
            bytes,
            bits,
            size,
        }))
    }

    pub fn from_field(field: &Field) -> Result<Option<Self>, syn::Error> {
        let mut args = SpecArgs::from_field(field)?;

        args.name = Some(args.name.unwrap_or(field.ident.clone().unwrap()));
        args.visibility = Some(args.visibility.unwrap_or(field.vis.clone()));
        args.mapped_type = Some(args.mapped_type.unwrap_or(field.ty.clone()));

        SpecField::new(args)
    }

    pub fn read(&self, buffer: TokenStream) -> TokenStream {
        let mut cursor = self.cursor;
        let mut offset = self.offset;

        let buffer_size = self.size;
        let bits = self.bits;

        let start = cursor;
        let end = cursor + buffer_size;

        if offset > 0 || (self.bits % 8 > 0) {
            let mut elements: Vec<TokenStream> = Vec::new();

            for i in 0..buffer_size {
                // total bits needed
                let need = if i > 0 || bits % 8 == 0 { 8 } else { bits % 8 };

                // bits to skip from the left
                let skip = offset + 0;

                // bits to keep in the current byte
                let keep = 8 - (8 - need).max(offset);

                // bits needed from the next byte
                let replace = need - keep;

                if replace > 0 {
                    elements.push(quote! {
	                    ((#buffer[#cursor] << #skip) >> (8 - #need)) | (#buffer[#cursor + 1] >> (8 - #replace))
	                });
                } else {
                    elements.push(quote! {
                        (#buffer[#cursor] << #skip) >> (8 - #need)
                    });
                }

                if replace > 0 || (offset + need) == 8 {
                    cursor += 1;
                    offset = replace;
                } else {
                    offset = (offset + need) % 8;
                }
            }

            return quote! { [ #(#elements),* ] };
        }

        quote! { &#buffer[#start..#end] }
    }

    pub fn map_read(&self, contents: TokenStream) -> TokenStream {
        let is_slice = contents.to_string().starts_with("&");

        let content_owned = if is_slice {
            quote! {
                (#contents).try_into().unwrap()
            }
        } else {
            quote! {
                #contents
            }
        };

        if let Some(map_expr) = self.map_read.clone() {
            if self.infallible_read {
                quote! {
                     (#map_expr)(#content_owned)
                }
            } else {
                quote! {
                     (#map_expr)(#content_owned)?
                }
            }
        } else {
            quote! {
                #contents
            }
        }
    }

    pub fn write(&self, contents: TokenStream, buffer: TokenStream) -> TokenStream {
        let cursor = self.cursor;

        quote! {
            {
                let _contents: [u8] = #contents;
                for i in 0.._mapped.len() {
                    #buffer[#cursor + i] = _mapped[i];
                }
            }
        }
    }

    pub fn map_write(&self, contents: TokenStream) -> TokenStream {
        if let Some(map_expr) = self.map_write.clone() {
            if self.infallible_read {
                quote! {
                     (#map_expr)(#contents)
                }
            } else {
                quote! {
                     (#map_expr)(#contents)?
                }
            }
        } else {
            quote! {
                #contents
            }
        }
    }

    pub fn assign(&self, contents: TokenStream, instance: Option<TokenStream>) -> TokenStream {
        let name = self.name.clone();

        let is_slice = self.map_read.is_none() && contents.to_string().starts_with("&");
        let want_slice = self.ty.to_token_stream().to_string().starts_with("&");

        let contents = if (want_slice && is_slice) || (!want_slice && !is_slice) {
            quote! {
                #contents
            }
        } else if want_slice && !is_slice {
            quote! {
                &#contents
            }
        } else {
            quote! {
                (#contents).try_into().unwrap()
            }
        };

        if let Some(instance) = instance {
            if self.vis.is_some() {
                return quote! {
                    #instance.#name = #contents
                };
            }
        }

        quote! {
            let #name = #contents
        }
    }

    pub fn define(&self) -> Option<TokenStream> {
        if let Some(vis) = self.vis.clone() {
            let name = self.name.clone();
            let attrs = self.attrs.clone();
            let ty = self.ty.clone();

            Some(quote! {
                #(#attrs)*
                #vis #name: #ty
            })
        } else {
            None
        }
    }
}

pub struct Spec {
    fields: Vec<SpecField>,
    count: usize,
    size: usize,
    bytes: usize,
    bits: usize,
}

impl Spec {
    pub fn new(fields: &Vec<Field>) -> Result<Self, syn::Error> {
        let mut spec = Spec {
            fields: Vec::new(),
            count: 0,
            bytes: 0,
            bits: 0,
            size: 0,
        };

        spec.append_fields(fields)?;

        Ok(spec)
    }

    pub fn append(&mut self, specs: Vec<SpecField>) {
        for field in specs {
            let mut spec = field;
            spec.index = self.count;
            self.fields.push(spec);
            self.count += 1;
        }

        self.align();
    }

    pub fn append_fields(&mut self, fields: &Vec<Field>) -> Result<(), syn::Error> {
        for field in fields {
            if let Some(spec) = SpecField::from_field(field)? {
                let mut spec = spec;
                spec.index = self.count;
                self.fields.push(spec);
                self.count += 1;
            }
        }

        self.align();
        Ok(())
    }

    pub fn push(&mut self, spec: SpecField) {
        let mut spec = spec;
        spec.index = self.count;
        self.fields.push(spec);
        self.count += 1;
        self.align()
    }

    pub fn push_field(&mut self, field: &Field) -> Result<(), syn::Error> {
        if let Some(spec) = SpecField::from_field(field)? {
            let mut spec = spec;
            spec.index = self.count;
            self.fields.push(spec);
            self.count += 1;
            self.align();
        }

        Ok(())
    }

    pub fn iter(&self) -> impl Iterator<Item = &SpecField> {
        self.fields.iter()
    }

    pub fn bits(&self) -> usize {
        self.bits
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn size(&self) -> usize {
        self.size
    }

    fn align(&mut self) {
        self.size = 0;
        self.bits = 0;
        self.bytes = 0;

        let mut cursor = 0;
        let mut offset = 0;
        let mut index = 0;

        let (mut pinned, mut unpinned): (Vec<_>, Vec<_>) = self.fields.drain(..).partition(|f| f.pinned);

        pinned.sort_by(|a, b| a.cursor.cmp(&b.cursor));
        unpinned.sort_by(|a, b| a.index.cmp(&b.index));

        let mut pinned = VecDeque::from(pinned);
        let mut unpinned = VecDeque::from(unpinned);

        if pinned.len() > 0 {
            while !pinned.is_empty() {
                let mut field = pinned.pop_front().unwrap();

                field.index = index;
                index += 1;

                if cursor == field.cursor {
                    field.offset = offset;
                } else {
                    field.offset = 0;
                }

                cursor = field.cursor + field.bytes + offset / 8;
                offset = (field.offset + field.bits) % 8;

                self.fields.push(field);

                if !pinned.is_empty() && !unpinned.is_empty() && pinned[0].cursor > cursor {
                    let mut field = unpinned.pop_front().unwrap();
                    
                    field.index = index;
                    index += 1;

                    field.cursor = cursor;
                    field.offset = offset;

                    cursor += field.bytes + (offset / 8);
                    offset = (offset + field.bits) % 8;

                    if cursor > pinned[0].cursor {
                        panic!("Field {} overlaps with pinned field {}", field.name, pinned[0].name);
                    }

                    self.fields.push(field);
                }
            }
        }

        while !unpinned.is_empty() {
            let mut field = unpinned.pop_front().unwrap();

            field.index = index;
            index += 1;

            field.cursor = cursor;
            field.offset = offset;

            cursor += field.bytes + ((offset + field.bits) / 8);
            offset = (offset + field.bits) % 8;

            self.fields.push(field);
        }

        self.size = cursor;
        self.bits = offset;
        self.bytes = if self.bits > 0 && cursor > 0 {
            cursor - 1
        } else {
            cursor
        };
    }
}
