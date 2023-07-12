use libnord::common;
use libnord::common::bank::Item;
use libnord::error::Error;
use libnord::types::RangedU16Pair;

#[test]
fn test_bank_can_replace_items() -> Result<(), Error> {
    const BANK_COUNT: u16 = 5;
    const SLOT_COUNT: u16 = 2;

    type Location = RangedU16Pair<BANK_COUNT, SLOT_COUNT>;
    type Bank = common::bank::Bank<TestItem, Location>;

    #[derive(Debug)]
    struct TestItem {
        pub location: Location,
        pub value: u16,
    }

    impl Item<Location> for TestItem {
        fn name(&self) -> Option<String> {
            Some("foo".to_string())
        }
        fn set_name(&mut self, _name: String) -> () {
            ()
        }
        fn location(&self) -> Location {
            self.location
        }
        fn set_location(&mut self, location: Location) -> () {
            self.location = location;
        }
    }

    let mut bank = Bank::new();

    bank.replace(TestItem {
        value: 69,
        location: (4, 1).try_into()?,
    });

    if let Some(result) = bank.get((4, 1).try_into()?) {
        assert_eq!(result.value, 69);
    } else {
        panic!("Expected to find item at (4,1) but found nothing");
    }

    assert_eq!(bank.get((0, 0).try_into()?).is_none(), true);

    Ok(())
}
