use libnord::types::RangedU16Pair;
use std::convert::TryInto;

#[test]
fn test_types_ranged_tuple_can_convert_to_u16() {
    let ranged_tuple: RangedU16Pair<5, 10> = (1, 2).try_into().unwrap();
    assert_eq!(ranged_tuple.as_u16(), 12);
}

#[test]
fn test_types_ranged_tuple_can_be_created_from_u16() {
    let ranged_tuple: RangedU16Pair<5, 10> = 12_u16.try_into().unwrap();
    assert_eq!(ranged_tuple, (1, 2));
}

#[test]
fn test_types_can_create_identity_point_in_empty_tuple_range() {
    let ranged_tuple: RangedU16Pair<0, 0> = 0_u16.try_into().unwrap();
    assert_eq!(ranged_tuple, (0, 0));
}
