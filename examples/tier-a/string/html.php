<?php
// html.c: htmlspecialchars / htmlentities / html_entity_decode /
// htmlspecialchars_decode / get_html_translation_table with every flag axis
// (quote style, doctype, ENT_SUBSTITUTE/IGNORE/DISALLOWED, double_encode)
// and the charsets php supports. Flag values are literal (constants are not
// lowered yet): ENT_COMPAT=2 ENT_QUOTES=3 ENT_NOQUOTES=0 ENT_IGNORE=4
// ENT_SUBSTITUTE=8 ENT_DISALLOWED=128 ENT_HTML401=0 ENT_XML1=16 ENT_XHTML=32
// ENT_HTML5=48 HTML_SPECIALCHARS=0 HTML_ENTITIES=1.

$s = "a & b < c > d \" e ' f";
foreach ([0, 16, 32, 48] as $doc) {
    foreach ([0, 2, 3] as $q) {
        echo htmlspecialchars($s, $q + $doc), "\n";
    }
}
var_dump(htmlspecialchars($s), htmlspecialchars("é€"), htmlspecialchars("x", 3, "UTF-8"), htmlspecialchars("x", 3, ""));

// --- malformed UTF-8: "" / U+FFFD / dropped ---
var_dump(htmlspecialchars("\xff abc"), htmlspecialchars("\xff abc", 3), htmlspecialchars("\xff abc", 3 + 4));
foreach (["\xc3", "\xc3(", "\xe2\x82", "\xe2\x82(", "\xf0\x9f\x98(", "\xc0\x80", "\xed\xa0\x80", "\xf4\x90\x80\x80", "\x80\x80", "\xf8\x88\x80\x80\x80", "a\xffb\xfec", "\xe0\x80\x80", "\xc1\xbf", "\xef\xbf\xbf", "\xf4\x8f\xbf\xbf"] as $bad) {
    echo bin2hex(htmlspecialchars($bad, 3 + 8)), "|", bin2hex(htmlspecialchars($bad, 3 + 4)), " ";
}
echo "\n";

// --- ENT_DISALLOWED ---
var_dump(bin2hex(htmlspecialchars("\x01\x7f\x80", 3 + 8 + 16 + 128)), bin2hex(htmlspecialchars("\x01\x7f\x08", 3 + 48 + 128)), bin2hex(htmlspecialchars("\x01\x08\x0b\x7f\x09", 3 + 128)), bin2hex(htmlspecialchars("\x01", 3 + 16)));
var_dump(htmlspecialchars("\x01<", 3 + 128, "ISO-8859-1"), htmlentities("\x01<\x80", 3 + 128, "ISO-8859-1"), htmlentities("\x80", 3 + 128, "cp1252"));

// --- double_encode = false ---
var_dump(htmlspecialchars("&amp; &lt; &#039; &#x1F; &#xZZ; &foo; &amp", 3, "UTF-8", false));
var_dump(htmlspecialchars("&apos; &Aacute; &NotEqualTilde; &amp;", 3, "UTF-8", false));
var_dump(htmlspecialchars("&apos; &Aacute; &NotEqualTilde; &amp;", 3 + 48, "UTF-8", false));
var_dump(htmlspecialchars("&apos; &Aacute; &amp;", 3 + 16, "UTF-8", false), htmlspecialchars("&apos; &Aacute; &amp;", 3 + 32, "UTF-8", false));
var_dump(htmlspecialchars("&#1;&#xD;&#x110000;&#x10FFFF;", 3 + 128, "UTF-8", false));
var_dump(htmlspecialchars("&#1;&#xD;&#xC;&#x110000;&#x10FFFF;&#x1F;", 3 + 48 + 128, "UTF-8", false));
var_dump(htmlspecialchars("&#1;&#xD;&#xC;&#x110000;&#x10FFFF;&#x1F;&#xFFFE;", 3 + 16 + 128, "UTF-8", false));
var_dump(htmlspecialchars("&#x110000;&#x10FFFF;&#1114111;&#1114112;&#0;&#x0;&#xD800;&#99999999999999999999;", 3, "UTF-8", false));
var_dump(htmlspecialchars("&quot;&#039;&apos;&#39;", 0, "UTF-8", false), htmlspecialchars("&quot;&#039;&apos;&#39;", 48, "UTF-8", false));
var_dump(htmlspecialchars("&amp;&AMP;&Amp;&lt;&LT;", 3 + 48, "UTF-8", false), htmlspecialchars("&amp;&AMP;&Amp;&lt;&LT;", 3, "UTF-8", false));
var_dump(htmlspecialchars("&amp &am;&1;&a-b;&amp; &am p;&&amp;&#&amp;&am&amp;", 3, "UTF-8", false));
var_dump(htmlspecialchars("&eacute;&euro;&#8364;&#x1F600;", 3, "ISO-8859-1", false));

// --- htmlentities ---
var_dump(htmlentities("café €"), htmlentities("café € ≠", 3 + 48), htmlentities("café € ≠ \xe2\x89\x82\xcc\xb8 \xe2\x89\x82x", 3 + 48), htmlentities("<a>", 3 + 16), htmlentities("é", 3 + 16), htmlentities("é", 3 + 32));
var_dump(htmlentities("\xe9", 3, "ISO-8859-1"), htmlentities("\xa4\xe9\xbc\x80", 3, "ISO-8859-1"), htmlentities("\xa4\xe9\xbc", 3, "ISO-8859-15"), htmlentities("\xa4\xa6\xb7", 3 + 48, "ISO-8859-15"), htmlentities("\xb7", 3, "ISO-8859-15"));
var_dump(htmlentities("\x80\x81\x93\xe9", 3 + 8, "cp1252"), htmlentities("\x80\x93\xe9", 3, "cp1252"), htmlentities("\x81", 3, "cp1252"), htmlentities("\x80\x8c\x9f\xa0", 3 + 48, "cp1252"));
var_dump(htmlentities("<\xe9>", 3, "KOI8-R"), htmlentities("\xc1\xd7", 3 + 48, "KOI8-R"), htmlentities("\xc1\xa8", 3 + 48, "cp1251"), htmlentities("\xa1\xf0\xfd\xff", 3 + 48, "iso8859-5"), htmlentities("\x80\xa4", 3 + 48, "cp866"), htmlentities("<\x80\xdb\xf0>", 3, "MacRoman"), htmlentities("\xa5\xd8", 3 + 48, "MacRoman"));
var_dump(htmlentities("é", 3, "ISO8859-1"), htmlentities("é", 3, "Windows-1252"), htmlentities("é", 3, "1252"), htmlentities("é", 3, "iso-8859-15"), htmlentities("é", 3, "IBM866"), htmlentities("é", 3, "MACROMAN"));
var_dump(htmlentities("é", 3, "utf8"));           // warning: charset not supported
var_dump(htmlentities("é", 3, "latin1"));         // warning
var_dump(htmlentities("<\xa4\xa1>", 3, "BIG5"));  // notice: basic substitution only
var_dump(htmlspecialchars("x<>", 3, "SJIS"));

// --- html_entity_decode: numeric entity validity per doctype ---
$tests = ["&#65;", "&#x41;", "&#X41;", "&#0;", "&#1;", "&#8;", "&#127;", "&#128;", "&#xD800;", "&#x110000;", "&#1114111;", "&#xFFFE;", "&#xFFFF;", "&#65", "&#;", "&#x;", "&#xZ;", "&#0065;", "&#99999999999999999999;", "&amp;", "&AMP;", "&amp", "&quot;", "&#039;", "&#39;", "&apos;", "&#x27;", "&#x22;", "&nbsp;", "&Nbsp;", "&hellip;", "&NotEqualTilde;", "&#x9;", "&#xA;", "&#xB;", "&#xC;", "&#xD;", "&#x1F;", "&#x7F;", "&#x80;", "&#x9F;", "&#xA0;", "&#xFDD0;", "&#x1FFFE;", "&#x10FFFF;", "&#xE000;", "&#x2028;", "&#38", "&#38;x", "&&amp;", "&am&amp;"];
foreach ([0, 16, 32, 48] as $doc) {
    $line = [];
    foreach ($tests as $t) {
        $line[] = bin2hex(html_entity_decode($t, 3 + $doc));
    }
    echo implode(" ", $line), "\n";
}
foreach (["&quot;", "&#039;", "&#39;", "&apos;", "&#x27;", "&#x22;"] as $t) {
    echo $t, ": ", html_entity_decode($t, 0), " ", html_entity_decode($t, 2), " ", html_entity_decode($t, 48), " ", html_entity_decode($t, 2 + 48), " | ", htmlspecialchars_decode($t, 0), " ", htmlspecialchars_decode($t, 2), " ", htmlspecialchars_decode($t, 3), " ", htmlspecialchars_decode($t, 3 + 48), " ", htmlspecialchars_decode($t, 3 + 16), "\n";
}
var_dump(htmlspecialchars_decode("&amp; &lt; &gt; &quot; &#039; &#39; &apos; &#x27; &#X27; &nbsp; &#65; &#x41; &amp;amp; &AMP; &Lt; &#0038; &#038; &#x026; &#38 &#38;"));
var_dump(htmlspecialchars_decode("&amp; &apos; &AMP; &#8364;&euro;&#x80;", 3 + 48));
var_dump(html_entity_decode("&eacute;", 3, "ISO-8859-1") === "\xe9", html_entity_decode("&euro;&#8364;&#233;&#xe9;", 3, "ISO-8859-1"), bin2hex(html_entity_decode("&#x1F600;&#xe9;&euro;&#8364;&Scaron;", 3, "ISO-8859-15")), bin2hex(html_entity_decode("&euro;&OElig;&Yuml;&#x81;", 3, "cp1252")), bin2hex(html_entity_decode("&Acy;&#x2500;", 3 + 48, "KOI8-R")));
var_dump(html_entity_decode("\xff&amp;"), html_entity_decode("\xff&amp;", 3 + 8));
var_dump(html_entity_decode("&#x110000;&#1;&#x80;", 3 + 128 + 48), html_entity_decode("&#x80;", 3 + 128));

// --- get_html_translation_table ---
foreach ([0, 16, 32, 48] as $doc) {
    $t = get_html_translation_table(1, 3 + $doc);
    echo "entities=", count($t), " special=", count(get_html_translation_table(0, 3 + $doc)), "\n";
    print_r(get_html_translation_table(0, 3 + $doc));
}
print_r(get_html_translation_table(0, 0 + 48));
print_r(get_html_translation_table(0, 2 + 48));
print_r(get_html_translation_table());
$t = get_html_translation_table(1, 3);
$i = 0;
foreach ($t as $k => $v) {
    if ($i < 9) {
        echo bin2hex($k), "=", $v, " ";
    }
    $i = $i + 1;
}
echo "\n";
$t5 = get_html_translation_table(1, 3 + 48);
$keys = array_keys($t5);
$i = array_search("≂", $keys);
$j = $i - 2;
while ($j < $i + 4) {
    echo bin2hex($keys[$j]), "=", $t5[$keys[$j]], " ";
    $j = $j + 1;
}
echo "\n";
var_dump(count(get_html_translation_table(1, 3, "ISO-8859-1")), count(get_html_translation_table(1, 3 + 48, "ISO-8859-1")), count(get_html_translation_table(1, 3 + 48, "cp1252")), count(get_html_translation_table(1, 0 + 48, "ISO-8859-15")), count(get_html_translation_table(1, 3 + 48, "KOI8-R")), count(get_html_translation_table(1, 3 + 48, "MacRoman")));
$t15 = get_html_translation_table(1, 0 + 48, "ISO-8859-15");
foreach ($t15 as $k => $v) {
    if (strlen($k) == 1 && ord($k) >= 0xa0 && ord($k) < 0xc0) {
        echo bin2hex($k), "=", $v, " ";
    }
}
echo "\n";
print_r(get_html_translation_table(0, 3, "ISO-8859-1"));
foreach (["ISO-8859-1", "cp1252", "MacRoman"] as $cs) {
    $t = get_html_translation_table(1, 3 + 48, $cs);
    echo $cs, " ", count($t), ": ";
    foreach ($t as $k => $v) {
        if (strlen($k) != 1 || ord($k) < 0x80) {
            echo bin2hex($k), "=", $v, " ";
        }
    }
    echo "\n";
}
var_dump(htmlentities("\t\n\x0d\x20x", 3 + 48, "MacRoman"), htmlentities("\t\n", 3 + 48, "cp1252"), htmlentities("fj", 3 + 48, "MacRoman"), htmlentities("fj", 3 + 48), htmlentities("<\xd2", 3 + 48, "ISO-8859-1"), bin2hex(html_entity_decode("&nvlt;&fjlig;", 3 + 48, "ISO-8859-1")), html_entity_decode("&Tab;", 3 + 48, "MacRoman") === "\t");
