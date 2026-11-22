use jiff::ToSpan as _;

use super::{CivilDate, Weekday};

pub(crate) fn days_in_month(year: i16, month: i8, first_day: Weekday) -> Vec<Vec<CivilDate>> {
    let (year, month) = match month {
        1..=12 => (year, month),
        13.. => (year + 1, 1),
        _ => (year - 1, 12),
    };

    let Ok(date) = CivilDate::new(year, month, 1) else {
        return Vec::new();
    };
    let offset = (date.weekday().to_sunday_zero_offset()
        - first_day.to_sunday_zero_offset())
        .rem_euclid(7);

    let days = (0..35)
        .map(|day| date.checked_add((day - i32::from(offset)).days()))
        .collect::<Result<Vec<_>, _>>();
    days.unwrap_or_default().chunks(7).map(<[_]>::to_vec).collect()
}

#[cfg(test)]
mod tests {
    use super::{CivilDate, Weekday};

    use super::days_in_month;

    #[test]
    fn test_days_in_month() {
        assert_eq!(
            CivilDate::new(2024, 2, 1).unwrap().days_in_month(),
            29
        );
        assert_eq!(
            CivilDate::new(2023, 2, 1).unwrap().days_in_month(),
            28
        );
        assert_eq!(
            CivilDate::new(2023, 1, 1).unwrap().days_in_month(),
            31
        );
        assert_eq!(
            CivilDate::new(2023, 4, 1).unwrap().days_in_month(),
            30
        );
    }

    #[test]
    fn test_days() {
        #[track_caller]
        fn assert_case(date: CivilDate, first_day: Weekday, expected: Vec<&str>) {
            let out = days_in_month(date.year(), date.month(), first_day)
                .iter()
                .map(|week| {
                    week.iter()
                        .map(|d| {
                            if d.year() == date.year() && d.month() == date.month() {
                                format!("{:2}", d.day())
                            } else if d.year() == date.year() {
                                format!("{}-{}", d.month(), d.day())
                            } else {
                                format!("{}-{}-{}", d.year(), d.month(), d.day())
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("|")
                })
                .collect::<Vec<_>>();

            assert_eq!(out, expected);
        }

        assert_case(
            CivilDate::new(2024, 8, 1).unwrap(),
            Weekday::Sunday,
            vec![
                "7-28|7-29|7-30|7-31| 1| 2| 3",
                " 4| 5| 6| 7| 8| 9|10",
                "11|12|13|14|15|16|17",
                "18|19|20|21|22|23|24",
                "25|26|27|28|29|30|31",
            ],
        );
        assert_case(
            CivilDate::new(2025, 1, 1).unwrap(),
            Weekday::Sunday,
            vec![
                "2024-12-29|2024-12-30|2024-12-31| 1| 2| 3| 4",
                " 5| 6| 7| 8| 9|10|11",
                "12|13|14|15|16|17|18",
                "19|20|21|22|23|24|25",
                "26|27|28|29|30|31|2-1",
            ],
        );

        assert_case(
            CivilDate::new(2024, 2, 1).unwrap(),
            Weekday::Sunday,
            vec![
                "1-28|1-29|1-30|1-31| 1| 2| 3",
                " 4| 5| 6| 7| 8| 9|10",
                "11|12|13|14|15|16|17",
                "18|19|20|21|22|23|24",
                "25|26|27|28|29|3-1|3-2",
            ],
        );
        assert_case(
            CivilDate::new(2023, 2, 20).unwrap(),
            Weekday::Sunday,
            vec![
                "1-29|1-30|1-31| 1| 2| 3| 4",
                " 5| 6| 7| 8| 9|10|11",
                "12|13|14|15|16|17|18",
                "19|20|21|22|23|24|25",
                "26|27|28|3-1|3-2|3-3|3-4",
            ],
        );

        // Monday-first week: Feb 2023 starts on a Wednesday, so the grid
        // shifts to start on Monday (column 0 = Mon 30 Jan).
        assert_case(
            CivilDate::new(2023, 2, 1).unwrap(),
            Weekday::Monday,
            vec![
                "1-30|1-31| 1| 2| 3| 4| 5",
                " 6| 7| 8| 9|10|11|12",
                "13|14|15|16|17|18|19",
                "20|21|22|23|24|25|26",
                "27|28|3-1|3-2|3-3|3-4|3-5",
            ],
        );
    }
}
