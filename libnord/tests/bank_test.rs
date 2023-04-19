use libnord::common::bank::{Bank, Coordinates, Item};

#[test]
fn test_can_replace_items_in_bank() {
    const BANK_COUNT: u16 = 5;
    const SLOT_COUNT: u16 = 2;

    type TestCoords = Coordinates<BANK_COUNT, SLOT_COUNT>;

    #[derive(Debug)]
    struct TestItem {
        pub location: TestCoords,
        pub value: u16,
    }

    impl Item<BANK_COUNT, SLOT_COUNT> for TestItem {
        fn name(&self) -> Option<String> {
            Some("foo".to_string())
        }

        fn set_name(&mut self, name: String) -> () {
            ()
        }

        fn location(&self) -> TestCoords {
            self.location
        }

        fn set_location(&mut self, location: TestCoords) -> () {
            self.location = location;
        }
    }

    let mut bank = Bank::<BANK_COUNT, SLOT_COUNT, TestItem>::new();

    bank.replace(TestItem {
        value: 69,
        location: (4, 1).into(),
    });

    if let Some(result) = bank.get((4, 1).into()) {
        assert_eq!(result.value, 69);
    } else {
        panic!("Expected to find item at (4,1) but found nothing");
    }

    assert_eq!(bank.get((0, 0).into()).is_none(), true);
}
