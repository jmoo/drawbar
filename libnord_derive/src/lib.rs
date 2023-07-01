use proc_macro::{self, TokenStream};

mod cbin;

#[proc_macro_attribute]
pub fn cbin(args: TokenStream, input: TokenStream) -> TokenStream {
    cbin::CBinGenerator::new(args.into(), input.into())
        .unwrap()
        .expand()
        .unwrap()
        .into()
}
