<?php
// IntlDateFormatter: the styles of many locales, every pattern field, the setters and the calendars.

$ts = 1700000000; // 2023-11-14 22:13:20 UTC
foreach (["en_US", "de_DE", "fr_FR", "ja_JP", "ar_EG", "en_GB", "es_ES", "zh_CN", "ru_RU", "hi_IN", "en_US_POSIX"] as $loc) {
    foreach ([[IntlDateFormatter::FULL, IntlDateFormatter::FULL], [IntlDateFormatter::LONG, IntlDateFormatter::LONG], [IntlDateFormatter::MEDIUM, IntlDateFormatter::MEDIUM], [IntlDateFormatter::SHORT, IntlDateFormatter::SHORT], [IntlDateFormatter::FULL, IntlDateFormatter::NONE], [IntlDateFormatter::NONE, IntlDateFormatter::SHORT], [IntlDateFormatter::MEDIUM, IntlDateFormatter::NONE], [IntlDateFormatter::SHORT, IntlDateFormatter::MEDIUM], [IntlDateFormatter::NONE, IntlDateFormatter::NONE]] as [$d, $t]) {
        $f = new IntlDateFormatter($loc, $d, $t, "UTC");
        echo "$loc $d/$t: ", json_encode($f->getPattern()), " => ", $f->format($ts), " | ", $f->format(0), " | ", $f->format(-86400 * 365 * 100), "\n";
    }
}
$f = new IntlDateFormatter("en_US", IntlDateFormatter::FULL, IntlDateFormatter::FULL, "Europe/Berlin");
var_dump($f->format($ts), $f->getTimeZoneId(), $f->getTimeZone()->getID(), $f->getCalendar(), $f->getDateType(), $f->getTimeType(), $f->getLocale(), $f->getLocale(Locale::VALID_LOCALE), $f->isLenient(), $f->getErrorCode(), $f->getErrorMessage(), $f->getCalendarObject()->getType(), $f->getCalendarObject()->getTimeZone()->getID());
$f = new IntlDateFormatter("en_US", IntlDateFormatter::SHORT, IntlDateFormatter::SHORT);
var_dump($f->getTimeZoneId(), $f->format($ts), date_default_timezone_get());
date_default_timezone_set("America/New_York");
$f = new IntlDateFormatter("en_US", IntlDateFormatter::SHORT, IntlDateFormatter::SHORT);
var_dump($f->getTimeZoneId(), $f->format($ts), $f->format(new DateTime("2024-03-10 14:30:00", new DateTimeZone("Asia/Tokyo"))), $f->format(new DateTimeImmutable("@0")), $f->format(1.5), $f->format("1700000000"), $f->format(["tm_year" => 124, "tm_mon" => 2, "tm_mday" => 10, "tm_hour" => 14, "tm_min" => 30, "tm_sec" => 5]), $f->format(["tm_year" => 124, "tm_mon" => 2, "tm_mday" => 10]), $f->format(IntlCalendar::fromDateTime("2024-03-10 14:30:00 Asia/Tokyo")));
date_default_timezone_set("UTC");
foreach (["yyyy-MM-dd HH:mm:ss zzzz", "EEEE, d MMMM yyyy 'at' h:mm a", "G GGGG GGGGG", "y yy yyy yyyy yyyyy", "M MM MMM MMMM MMMMM", "L LL LLL LLLL LLLLL", "d dd D DD DDD", "E EE EEE EEEE EEEEE EEEEEE", "c cc ccc cccc ccccc cccccc", "e ee eee eeee eeeee eeeeee", "a aa aaa aaaa aaaaa", "b bbbb B BBBB", "h hh H HH k kk K KK", "m mm s ss S SS SSS SSSS", "z zz zzz zzzz", "Z ZZ ZZZ ZZZZ ZZZZZ", "v vvvv V VV VVV VVVV", "O OOOO", "X XX XXX XXXX XXXXX", "x xx xxx xxxx xxxxx", "Q QQ QQQ QQQQ q qq qqq qqqq", "w ww W", "u uu uuuu U UUUU", "r rrrr", "F g A", "Y YY YYYY", "'literal' '' 'it''s'", "j jj J C", "Ec", "yMd", "MMMMEEEEd"] as $pat) {
    $f = new IntlDateFormatter("en_US", IntlDateFormatter::NONE, IntlDateFormatter::NONE, "America/New_York", null, $pat);
    echo json_encode($pat), " => ", json_encode($f->getPattern()), " ", $f->format($ts), " | ", $f->format(86400 * 45 + 3661.5), "\n";
}
