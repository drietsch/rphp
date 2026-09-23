<?php
// ext/xml: xml_parse_into_struct()'s values and index arrays, and the
// options that shape them.
$doc = "<?xml version='1.0'?>\n<!DOCTYPE r [<!ENTITY e 'ent'>]>\n<root a='1' b=\"x&amp;y\">\n"
    . "  <item id='1'>Hello &amp; &e; &#65;</item>\n  <!-- c -->\n  <?pi data?>\n"
    . "  <![CDATA[ raw <x> ]]>\n  <empty/>\n <ns:x xmlns:ns='urn:a'>t</ns:x>tail</root>";
$p = xml_parser_create();
var_dump(xml_parse_into_struct($p, $doc, $values, $index));
print_r($values);
print_r($index);

echo "--- without an index\n";
var_dump(xml_parse_into_struct(xml_parser_create(), "<a><b>1</b><b>2</b></a>", $v));
print_r($v);

echo "--- skip white\n";
$p = xml_parser_create();
xml_parser_set_option($p, XML_OPTION_SKIP_WHITE, 1);
xml_parse_into_struct($p, "<a>\n  <b> x </b>\n  <c>  </c> <d>\n</d>t</a>", $v);
print_r($v);

echo "--- skip tag start, no case folding\n";
$p = xml_parser_create();
xml_parser_set_option($p, XML_OPTION_SKIP_TAGSTART, 2);
xml_parser_set_option($p, XML_OPTION_CASE_FOLDING, 0);
xml_parse_into_struct($p, "<abc x='1'><d>t</d>u</abc>", $v, $i);
print_r($v);
print_r($i);

echo "--- target encodings\n";
foreach (["UTF-8", "ISO-8859-1", "US-ASCII"] as $target) {
    $p = xml_parser_create();
    xml_parser_set_option($p, XML_OPTION_TARGET_ENCODING, $target);
    xml_parse_into_struct($p, "<a b='\xc3\xa9'>\xc3\xa9x\xe2\x82\xac</a>", $v);
    echo $target, " ", xml_parser_get_option($p, XML_OPTION_TARGET_ENCODING), " ",
        bin2hex($v[0]['value']), " ", bin2hex($v[0]['attributes']['B']), "\n";
}
$p = xml_parser_create("ISO-8859-1");
var_dump(xml_parser_get_option($p, XML_OPTION_TARGET_ENCODING));
xml_parse_into_struct($p, "<?xml version='1.0' encoding='ISO-8859-1'?><a>\xe9</a>", $v);
var_dump(bin2hex($v[0]['value']));

echo "--- a broken document\n";
$p = xml_parser_create();
var_dump(xml_parse_into_struct($p, "<a><b>text</a>", $v, $i), xml_get_error_code($p));
print_r($v);
print_r($i);

echo "--- handlers still run\n";
$p = xml_parser_create();
xml_set_element_handler($p, function ($p, $n) { echo "start $n\n"; }, null);
xml_parse_into_struct($p, "<a><b/></a>", $v);
echo count($v), "\n";
