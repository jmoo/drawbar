use libnord_codegen::binrw::{Spec, SpecField, SpecArgs};
use syn::parse_quote;
 
#[test]
fn test_binrw_initialize_empty_spec() {
	Spec::new(&vec![]).unwrap();
}

#[test]
fn test_binrw_align_increments_cursor() {
	let mut spec = Spec::new(&vec![]).unwrap();

	spec.append(vec![
		SpecField::new(SpecArgs {
			name: Some(parse_quote! { a }),
			mapped_type: Some(parse_quote! { u8 }),
			..Default::default()
		}).unwrap().unwrap(),

		SpecField::new(SpecArgs {
			name: Some(parse_quote! { b }),
			mapped_type: Some(parse_quote! { u8 }),
			..Default::default()
		}).unwrap().unwrap(),
	]);

	let fields = spec.iter().collect::<Vec<_>>();

	assert_eq!(fields.len(), 2);
	assert_eq!(fields[0].cursor, 0);
	assert_eq!(fields[1].cursor, 1);
}
