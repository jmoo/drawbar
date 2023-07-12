use proc_macro::{self, TokenStream};
use libnord_codegen::cbin;

#[proc_macro_attribute]
pub fn cbin(args: TokenStream, input: TokenStream) -> TokenStream {
    cbin::Generator::new(args.into(), input.into())
        .unwrap()
        .expand()
        .unwrap()
        .into()
}
