<?php
// Tier-A differential: idn_to_ascii / idn_to_utf8 — UTS #46 processing with
// ICU's error bits in $idna_info, transitional vs nontransitional handling
// of the deviation characters, and the argument errors.
$inputs = ["example.com", "münchen.de", "bücher.example", "xn--mnchen-3ya.de", ".", "a..b", "-abc.com", "abc-.com", "ab--cd.com", "xn--abc.com",
    str_repeat("a", 64) . ".com", str_repeat("a", 63) . ".com", "faß.de", "ΣΌΛΟΣ.gr", "日本語.jp", "a\u{200D}b.com", "☃.net", "ex ample.com", "exam_ple.com",
    "EXAMPLE.COM", "xn--", "xn--a.com", "a.b.c.example", "тест.рф", "u\u{0308}ber.de", "a.b.", "XN--MNCHEN-3YA.de", "\u{0301}abc.com", "a\u{FFFD}b.com", "x.xn--80ak6aa92e.com", "xn--80ak6aa92e"];
foreach ($inputs as $d) {
    $info = null; $a = idn_to_ascii($d, IDNA_DEFAULT, INTL_IDNA_VARIANT_UTS46, $info);
    $info2 = null; $u = idn_to_utf8($d, IDNA_DEFAULT, INTL_IDNA_VARIANT_UTS46, $info2);
    echo json_encode([$d, $a, $info, $u, $info2, intl_get_error_code()]), "\n";
}
var_dump(idn_to_ascii("faß.de", IDNA_NONTRANSITIONAL_TO_ASCII), idn_to_ascii("faß.de", 0), idn_to_utf8("faß.de", 0), idn_to_ascii("exam_ple.com", IDNA_USE_STD3_RULES), idn_to_ascii("a\u{200D}b.com", IDNA_CHECK_CONTEXTJ), idn_to_ascii("münchen.de"), idn_to_utf8("xn--mnchen-3ya.de"));
$info = null; var_dump(idn_to_ascii(str_repeat("a", 63) . "." . str_repeat("b", 63) . "." . str_repeat("c", 63) . "." . str_repeat("d", 63), IDNA_DEFAULT, INTL_IDNA_VARIANT_UTS46, $info), $info);
try { idn_to_ascii(""); } catch (ValueError $e) { echo $e->getMessage(), "\n"; }
try { idn_to_ascii("a.com", IDNA_DEFAULT, 0); } catch (ValueError $e) { echo $e->getMessage(), "\n"; }
