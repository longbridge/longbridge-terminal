use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use unicode_width::UnicodeWidthChar;

pub fn align_right(text: &str, width: usize) -> String {
    let extra: usize = text
        .chars()
        .filter_map(|c| c.width_cjk().and_then(|w| w.checked_sub(1)))
        .sum();
    format!(
        "{text:>width$}",
        width = width.checked_sub(extra).unwrap_or(width)
    )
}

pub fn unit(number: Decimal, precision: u32) -> String {
    match rust_i18n::locale().as_str() {
        "zh-CN" => unit_4(number, precision, (" 万", " 亿", " 万亿")),
        "zh-HK" => unit_4(number, precision, (" 萬", " 億", " 萬億")),
        _ => unit_3(number, precision, ("K", "M", "B")),
    }
}

fn unit_4(number: Decimal, precision: u32, units: (&str, &str, &str)) -> String {
    if number >= dec!(1e12) {
        return format!("{}{}", (number / dec!(1e12)).round_dp(precision), units.2);
    }
    if number >= dec!(1e8) {
        return format!("{}{}", (number / dec!(1e8)).round_dp(precision), units.1);
    }
    if number >= dec!(1e4) {
        return format!("{}{}", (number / dec!(1e4)).round_dp(precision), units.0);
    }
    format!("{}", number.round_dp(precision))
}

fn unit_3(number: Decimal, precision: u32, units: (&str, &str, &str)) -> String {
    if number >= dec!(1e9) {
        return format!("{}{}", (number / dec!(1e9)).round_dp(precision), units.2);
    }
    if number >= dec!(1e6) {
        return format!("{}{}", (number / dec!(1e6)).round_dp(precision), units.1);
    }
    if number >= dec!(1e3) {
        return format!("{}{}", (number / dec!(1e3)).round_dp(precision), units.0);
    }
    format!("{}", number.round_dp(precision))
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_align_right() {
        assert_eq!(align_right("text", 3), "text");
        assert_eq!(align_right("text", 10), "      text");
        // CJK characters
        assert_eq!(align_right("你好世界", 3), "你好世界");
        assert_eq!(align_right("你好世界", 10), "  你好世界");
    }

    #[test]
    fn test_unit() {
        rust_i18n::set_locale("en");
        assert_eq!(unit(dec!(1), 2), "1");
        assert_eq!(unit(dec!(1), 0), "1");
        assert_eq!(unit(dec!(12), 0), "12");
        assert_eq!(unit(dec!(2300), 0), "2K");
        assert_eq!(unit(dec!(2300), 1), "2.3K");
        assert_eq!(unit(dec!(232300), 2), "232.30K");
        assert_eq!(unit(dec!(78232300), 2), "78.23M");
        assert_eq!(unit(dec!(29278232300), 0), "29B");

        // Test in Chinese
        rust_i18n::set_locale("zh-CN");
        assert_eq!(unit(dec!(1), 2), "1");
        assert_eq!(unit(dec!(1), 0), "1");
        assert_eq!(unit(dec!(12), 0), "12");
        assert_eq!(unit(dec!(23000), 0), "2 万");
        assert_eq!(unit(dec!(23000), 1), "2.3 万");
        assert_eq!(unit(dec!(232300), 2), "23.23 万");
        assert_eq!(unit(dec!(782323000), 2), "7.82 亿");
        assert_eq!(unit(dec!(2927823230000), 0), "3 万亿");
        assert_eq!(unit(dec!(2927823230000), 2), "2.93 万亿");
    }
}
