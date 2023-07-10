use proc_macro::{self, TokenStream};

mod cbin;
pub(crate) mod spec;

#[proc_macro_attribute]
pub fn cbin(args: TokenStream, input: TokenStream) -> TokenStream {
    cbin::Generator::new(args.into(), input.into())
        .unwrap()
        .expand()
        .unwrap()
        .into()
}
