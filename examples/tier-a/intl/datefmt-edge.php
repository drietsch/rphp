<?php
// IntlDateFormatter, part two: odd patterns, the setters, calendars, relative styles, formatObject, parsing.
date_default_timezone_set("UTC");
$ts = 1700000000;

// setPattern / errors / lenient / calendar / timezone objects
$f = new IntlDateFormatter("en_US", IntlDateFormatter::MEDIUM, IntlDateFormatter::SHORT, "UTC");
var_dump($f->setPattern("dd.MM.yyyy"), $f->getPattern(), $f->format($ts), $f->setPattern(""), $f->getPattern(), $f->format($ts), $f->setPattern("'unterminated"), $f->getErrorMessage(), $f->getPattern(), $f->setTimeZone("Asia/Tokyo"), $f->getTimeZoneId(), $f->format($ts), $f->setTimeZone(new DateTimeZone("Europe/Paris")), $f->getTimeZoneId(), $f->setTimeZone(IntlTimeZone::createTimeZone("America/Sao_Paulo")), $f->getTimeZoneId(), $f->setTimeZone(null), $f->getTimeZoneId(), $f->setLenient(false), $f->isLenient(), $f->setCalendar(IntlDateFormatter::TRADITIONAL), $f->getCalendar(), $f->getCalendarObject()->getType(), $f->setCalendar(IntlDateFormatter::GREGORIAN), $f->getCalendar(), $f->getCalendar()); try { var_dump($f->setCalendar(99)); } catch (Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; } foreach (["Nope/Zone", "+02:00", "GMT+5:30", "EST", "Z", "utc", "GMT", "Etc/GMT+3", "America/Argentina/Buenos_Aires", "US/Pacific", "CET", "gmt+02:00", "GMT+0200", "UTC+2"] as $z) try { var_dump($f->setTimeZone($z), $f->getTimeZoneId(), $f->format($ts)); } catch (Throwable $e) { echo get_class($e), ": ", $e->getMessage(), " ", $f->getErrorCode(), " ", intl_get_error_code(), "\n"; }
foreach ([[null, "Nope/Zone"], ["", "UTC"], ["en", null], ["xx", null], ["en_US", "GMT+1"], ["fa_IR", "UTC"]] as [$loc, $tz]) { try { $f = new IntlDateFormatter($loc, IntlDateFormatter::MEDIUM, IntlDateFormatter::SHORT, $tz); echo json_encode($loc), "/", json_encode($tz), ": ", json_encode($f->getPattern()), " ", $f->format($ts), " tz=", $f->getTimeZoneId(), " cal=", $f->getCalendar(), "/", $f->getCalendarObject()->getType(), " loc=", $f->getLocale(), " err=", $f->getErrorCode(), "\n"; } catch (Throwable $e) { echo json_encode($loc), "/", json_encode($tz), ": ", get_class($e), ": ", $e->getMessage(), " intl=", intl_get_error_code(), "\n"; } }
var_dump(datefmt_create("en", IntlDateFormatter::SHORT, IntlDateFormatter::SHORT, "Nope/Zone"), intl_get_error_message(), datefmt_create("en", 99, 99), intl_get_error_message());
// relative
$f = new IntlDateFormatter("en_US", IntlDateFormatter::RELATIVE_FULL, IntlDateFormatter::NONE, "UTC"); $now = time(); echo json_encode($f->getPattern()), " ", $f->format($now), " | ", $f->format($now - 86400), " | ", $f->format($now + 86400), " | ", $f->format($now + 2 * 86400), " | ", $f->format($ts), "\n";
foreach ([IntlDateFormatter::RELATIVE_LONG, IntlDateFormatter::RELATIVE_MEDIUM, IntlDateFormatter::RELATIVE_SHORT] as $s) { $f = new IntlDateFormatter("en_US", $s, IntlDateFormatter::SHORT, "UTC"); echo $s, ": ", json_encode($f->getPattern()), " ", $f->format($now), " | ", $f->format($now - 86400), " | ", $f->format($ts), "\n"; }
$f = new IntlDateFormatter("de_DE", IntlDateFormatter::RELATIVE_FULL, IntlDateFormatter::SHORT, "UTC"); echo $f->format($now), " | ", $f->format($now + 86400), "\n";
// formatObject
var_dump(IntlDateFormatter::formatObject(new DateTime("2024-03-10 14:30:00 UTC")), IntlDateFormatter::formatObject(new DateTime("2024-03-10 14:30:00 Asia/Tokyo"), IntlDateFormatter::SHORT), IntlDateFormatter::formatObject(new DateTime("2024-03-10 14:30:00 Asia/Tokyo"), [IntlDateFormatter::LONG, IntlDateFormatter::NONE], "de"), IntlDateFormatter::formatObject(new DateTime("2024-03-10 14:30:00 Asia/Tokyo"), "EEEE dd MMM yyyy zzzz", "fr"), IntlDateFormatter::formatObject(new DateTime("2024-03-10 14:30:00 UTC"), null, "ja"), datefmt_format_object(new DateTimeImmutable("2024-03-10 14:30:00 +05:30"), IntlDateFormatter::FULL));


try { var_dump(datefmt_create("en", IntlDateFormatter::SHORT, IntlDateFormatter::SHORT, "Nope/Zone")); } catch (Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; } var_dump(intl_get_error_message());
try { var_dump(datefmt_create("en", 99, 99)); } catch (Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; } var_dump(intl_get_error_message());
try { var_dump(new IntlDateFormatter("en", 99, 99)); } catch (Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { var_dump(new IntlDateFormatter("en", IntlDateFormatter::SHORT, IntlDateFormatter::SHORT, "UTC", 99)); } catch (Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { var_dump(new IntlDateFormatter("en", IntlDateFormatter::SHORT, IntlDateFormatter::SHORT, "UTC", null, "'x")->getPattern()); } catch (Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
$f = new IntlDateFormatter("en", IntlDateFormatter::SHORT, IntlDateFormatter::SHORT, "UTC", IntlCalendar::createInstance("Asia/Tokyo", "en")); var_dump($f->getTimeZoneId(), $f->getCalendar(), $f->getCalendarObject()->getTimeZone()->getID(), $f->format($ts));
$f = new IntlDateFormatter("en", IntlDateFormatter::SHORT, IntlDateFormatter::SHORT, null, IntlCalendar::createInstance("Asia/Tokyo", "en")); var_dump($f->getTimeZoneId(), $f->format($ts));
$f = new IntlDateFormatter("en", IntlDateFormatter::SHORT, IntlDateFormatter::SHORT, "Europe/Berlin", IntlDateFormatter::TRADITIONAL); var_dump($f->getCalendar(), $f->getCalendarObject()->getType(), $f->format($ts));
$f = new IntlDateFormatter("ar_SA", IntlDateFormatter::MEDIUM, IntlDateFormatter::SHORT, "UTC", IntlDateFormatter::TRADITIONAL); var_dump($f->getCalendar(), $f->getCalendarObject()->getType(), $f->getPattern(), $f->format($ts));
$f = new IntlDateFormatter("th_TH", IntlDateFormatter::MEDIUM, IntlDateFormatter::SHORT, "UTC", IntlDateFormatter::TRADITIONAL); var_dump($f->getCalendar(), $f->getCalendarObject()->getType(), $f->getPattern(), $f->format($ts));
$f = new IntlDateFormatter("ja_JP", IntlDateFormatter::MEDIUM, IntlDateFormatter::SHORT, "UTC", IntlDateFormatter::TRADITIONAL); var_dump($f->getCalendar(), $f->getCalendarObject()->getType(), $f->getPattern(), $f->format($ts));
// relative
$f = new IntlDateFormatter("en_US", IntlDateFormatter::RELATIVE_FULL, IntlDateFormatter::NONE, "UTC"); $now = 1758513600; echo json_encode($f->getPattern()), " ", $f->format($now), " | ", $f->format($now - 86400), " | ", $f->format($now + 86400), " | ", $f->format($now + 2 * 86400), " | ", $f->format($ts), " dt=", $f->getDateType(), "\n";
foreach ([IntlDateFormatter::RELATIVE_LONG, IntlDateFormatter::RELATIVE_MEDIUM, IntlDateFormatter::RELATIVE_SHORT] as $s) { $f = new IntlDateFormatter("en_US", $s, IntlDateFormatter::SHORT, "UTC"); echo $s, ": ", json_encode($f->getPattern()), " ", $f->format($now), " | ", $f->format($now - 86400), " | ", $f->format($ts), "\n"; }
$f = new IntlDateFormatter("de_DE", IntlDateFormatter::RELATIVE_FULL, IntlDateFormatter::SHORT, "UTC"); echo $f->format($now), " | ", $f->format($now + 86400), " | ", $f->format($now - 2 * 86400), "\n";
$f = new IntlDateFormatter("en_US", IntlDateFormatter::RELATIVE_FULL, IntlDateFormatter::SHORT, "UTC"); var_dump($f->setPattern("yyyy"), $f->format($now), $f->getPattern());
// formatObject
var_dump(IntlDateFormatter::formatObject(new DateTime("2024-03-10 14:30:00 UTC")), IntlDateFormatter::formatObject(new DateTime("2024-03-10 14:30:00 Asia/Tokyo"), IntlDateFormatter::SHORT), IntlDateFormatter::formatObject(new DateTime("2024-03-10 14:30:00 Asia/Tokyo"), [IntlDateFormatter::LONG, IntlDateFormatter::NONE], "de"), IntlDateFormatter::formatObject(new DateTime("2024-03-10 14:30:00 Asia/Tokyo"), "EEEE dd MMM yyyy zzzz", "fr"), IntlDateFormatter::formatObject(new DateTime("2024-03-10 14:30:00 UTC"), null, "ja"), datefmt_format_object(new DateTimeImmutable("2024-03-10 14:30:00 +05:30"), IntlDateFormatter::FULL), IntlDateFormatter::formatObject(new DateTime("2024-03-10 14:30:00 EST"), IntlDateFormatter::FULL), IntlDateFormatter::formatObject(new DateTime("2024-03-10 14:30:00 America/New_York"), IntlDateFormatter::FULL), IntlDateFormatter::formatObject(new DateTime("2024-07-10 14:30:00 America/New_York"), IntlDateFormatter::FULL));
try { IntlDateFormatter::formatObject(new DateTime(), 99); } catch (Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { IntlDateFormatter::formatObject(new DateTime(), [1]); } catch (Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { IntlDateFormatter::formatObject("x"); } catch (Throwable $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
var_dump(IntlDateFormatter::formatObject(new DateTime(), ""), intl_get_error_message());


$f = new IntlDateFormatter("en_US", IntlDateFormatter::MEDIUM, IntlDateFormatter::SHORT, "UTC");
foreach (["Nov 14, 2023, 10:13 PM", "Nov 14, 2023, 10:13 pm", "nov 14, 2023, 10:13 PM", "November 14, 2023, 10:13 PM", "Nov 14, 2023 10:13 PM", "Nov 14, 2023,10:13 PM", "Nov 14, 2023, 22:13", "Nov 14, 2023, 10:13", "Nov 14 2023, 10:13 PM", "Nov 14, 23, 10:13 PM", "Nov 14, 2023, 10:13 PM extra", "  Nov 14, 2023, 10:13 PM", "Nov 14, 2023", "14 Nov 2023, 10:13 PM", "11/14/23, 10:13 PM", "Nov 31, 2023, 10:13 PM", "Nov 14, 2023, 25:13 PM", "Nov 14, 2023, 10:73 PM", "Nov 14, 2023, 10:13 XM", "", "Nov"] as $s) { $p = 0; $r = $f->parse($s, $p); echo json_encode($s), " => ", var_export($r, true), " @$p err=", $f->getErrorCode(), " ", $f->getErrorMessage(), "\n"; }
$f->setLenient(false);
echo "--strict\n";
foreach (["Nov 14, 2023, 10:13 PM", "nov 14, 2023, 10:13 PM", "November 14, 2023, 10:13 PM", "Nov 14, 2023 10:13 PM", "Nov 14, 2023, 22:13", "Nov 14, 23, 10:13 PM", "Nov 14, 2023, 10:13 PM extra", "  Nov 14, 2023, 10:13 PM", "Nov 31, 2023, 10:13 PM", "Nov 14, 2023, 10:13 PM"] as $s) { $p = 0; $r = $f->parse($s, $p); echo json_encode($s), " => ", var_export($r, true), " @$p err=", $f->getErrorCode(), "\n"; }
$f->setLenient(true);
$p = 3; var_dump($f->parse("xx Nov 14, 2023, 10:13 PM", $p), $p);
$p = 99; var_dump($f->parse("Nov 14, 2023, 10:13 PM", $p), $p, $f->getErrorCode());
$p = -1; var_dump($f->parse("Nov 14, 2023, 10:13 PM", $p), $p, $f->getErrorCode());
var_dump($f->localtime("Nov 14, 2023, 10:13 PM"), $f->localtime("bad"), $f->getErrorCode());
$p = 0; var_dump($f->localtime("Nov 14, 2023, 10:13 PM", $p), $p);
$g = new IntlDateFormatter("de_DE", IntlDateFormatter::NONE, IntlDateFormatter::NONE, "Europe/Berlin", null, "dd.MM.yyyy HH:mm:ss");
foreach (["14.11.2023 22:13:20", "14.11.2023", "1.1.2023 1:2:3", "14.11.2023 22:13:20.5", "14.11.23 22:13:20", "14.11.2023 22:13:20 UTC", "31.3.2024 02:30:00"] as $s) { $p = 0; $r = $g->parse($s, $p); echo json_encode($s), " => ", var_export($r, true), " @$p err=", $g->getErrorCode(), "\n"; }
$h = new IntlDateFormatter("en_US", IntlDateFormatter::NONE, IntlDateFormatter::NONE, "UTC", null, "yyyy-MM-dd'T'HH:mm:ssXXX");
foreach (["2023-11-14T22:13:20Z", "2023-11-14T22:13:20+01:00", "2023-11-14T22:13:20-0500", "2023-11-14T22:13:20"] as $s) { $p = 0; $r = $h->parse($s, $p); echo json_encode($s), " => ", var_export($r, true), " @$p err=", $h->getErrorCode(), "\n"; }
$i = new IntlDateFormatter("en_US", IntlDateFormatter::NONE, IntlDateFormatter::NONE, "UTC", null, "EEEE, MMMM d, y 'at' h:mm:ss a zzzz");
foreach (["Tuesday, November 14, 2023 at 10:13:20 PM Coordinated Universal Time", "Tuesday, November 14, 2023 at 10:13:20 PM UTC", "Tuesday, November 14, 2023 at 10:13:20 PM Eastern Standard Time", "Tuesday, November 14, 2023 at 10:13:20 PM EST", "Tuesday, November 14, 2023 at 10:13:20 PM GMT+2", "Tuesday, November 14, 2023 at 10:13:20 PM GMT+02:00", "Wednesday, November 14, 2023 at 10:13:20 PM UTC", "November 14, 2023 at 10:13:20 PM UTC"] as $s) { $p = 0; $r = $i->parse($s, $p); echo json_encode($s), " => ", var_export($r, true), " @$p err=", $i->getErrorCode(), "\n"; }
$j = new IntlDateFormatter("en_US", IntlDateFormatter::NONE, IntlDateFormatter::NONE, "UTC", null, "yy MM dd");
foreach (["23 11 14", "99 11 14", "50 11 14", "24 11 14", "2023 11 14", "1 11 14", "30 11 14", "31 11 14"] as $s) { $p = 0; $r = $j->parse($s, $p); echo json_encode($s), " => ", var_export($r, true), " @$p ", $r === false ? "" : gmdate("Y-m-d", $r), "\n"; }
$k = new IntlDateFormatter("en_US", IntlDateFormatter::NONE, IntlDateFormatter::NONE, "UTC", null, "y M d");
foreach (["2023 11 14", "23 11 14", "2023 1 4", "20231114", "2023 13 14", "2023 11 40", "0 1 1", "-5 1 1"] as $s) { $p = 0; $r = $k->parse($s, $p); echo json_encode($s), " => ", var_export($r, true), " @$p ", $r === false ? "" : gmdate("Y-m-d", $r), "\n"; }
$l = new IntlDateFormatter("en_US", IntlDateFormatter::NONE, IntlDateFormatter::NONE, "UTC", null, "yyyyMMddHHmm");
foreach (["202311142213", "2023111422", "20231114"] as $s) { $p = 0; $r = $l->parse($s, $p); echo json_encode($s), " => ", var_export($r, true), " @$p\n"; }
$m = new IntlDateFormatter("en_US", IntlDateFormatter::NONE, IntlDateFormatter::NONE, "UTC", null, "h:mm a");
foreach (["10:13 PM", "10:13 AM", "12:00 AM", "12:00 PM", "10:13", "0:13 AM", "13:13 PM"] as $s) { $p = 0; $r = $m->parse($s, $p); echo json_encode($s), " => ", var_export($r, true), " @$p ", $r === false ? "" : gmdate("Y-m-d H:i:s", $r), "\n"; }
$n = new IntlDateFormatter("en_US", IntlDateFormatter::NONE, IntlDateFormatter::NONE, "UTC", null, "MMMM d");
foreach (["November 14", "Nov 14", "N 14", "november 14", "Novem 14", "Feb 30"] as $s) { $p = 0; $r = $n->parse($s, $p); echo json_encode($s), " => ", var_export($r, true), " @$p ", $r === false ? "" : gmdate("Y-m-d H:i:s", $r), "\n"; }
