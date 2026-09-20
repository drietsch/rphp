<?php
// Text merging (none), whitespace handling with `preserveWhiteSpace`,
// `formatOutput` indentation, libxml's diagnostics through
// `libxml_use_internal_errors()` and as warnings, internal entities kept as
// entity references unless `substituteEntities`.
$d = new DOMDocument();
$e = $d->createElement('e'); $d->appendChild($e);
$e->appendChild($d->createTextNode('a')); $e->appendChild($d->createTextNode('b'));
var_dump($e->childNodes->length, $e->firstChild->nodeValue, $e->firstChild->nextSibling?->nodeValue);
$e->appendChild($d->createElement('x')); $e->appendChild($d->createTextNode('c')); $e->appendChild($d->createTextNode('d'));
var_dump($e->childNodes->length);
echo $d->saveXML();
// whitespace handling
$d2 = new DOMDocument(); $d2->loadXML("<r>\n  <a>1</a>\n  <b/>\n</r>");
var_dump($d2->documentElement->childNodes->length, $d2->documentElement->firstChild->nodeType);
$d2->formatOutput = true; echo $d2->saveXML();
$d3 = new DOMDocument(); $d3->preserveWhiteSpace = false; $d3->loadXML("<r>\n  <a>1</a>\n  <b/>\n</r>");
var_dump($d3->documentElement->childNodes->length); $d3->formatOutput = true; echo $d3->saveXML();
$d4 = new DOMDocument(); $d4->loadXML('<r><a><b>x</b></a><c/></r>'); $d4->formatOutput = true; echo $d4->saveXML(); echo $d4->saveXML($d4->documentElement), "|\n";
// errors
libxml_use_internal_errors(true);
$d5 = new DOMDocument();
var_dump($d5->loadXML('<r><a></r>'));
foreach (libxml_get_errors() as $err) { var_dump($err); }
libxml_clear_errors();
var_dump(libxml_get_last_error());
libxml_clear_errors();
var_dump($d5->loadXML('not xml'), count(libxml_get_errors()), libxml_get_errors()[0]->message, libxml_get_errors()[0]->code, libxml_get_errors()[0]->level, libxml_get_errors()[0]->line, libxml_get_errors()[0]->column);
libxml_clear_errors();
var_dump($d5->loadXML('<r xmlns:p="u"><q:a/></r>'));
foreach (libxml_get_errors() as $err) { echo $err->level, " ", $err->code, " ", $err->message, "| line ", $err->line, " col ", $err->column, "\n"; }
libxml_clear_errors();
libxml_use_internal_errors(false);
var_dump($d5->loadXML('<r><a></r>'));
$d5->loadXML('<a>&foo;</a>');
echo "---\n";
$d6 = new DOMDocument(); $d6->loadXML('<!DOCTYPE r [<!ENTITY foo "bar">]><r>&foo; &amp; &#65; &#x42;</r>');
var_dump($d6->documentElement->childNodes->length, $d6->documentElement->firstChild->nodeType, $d6->documentElement->textContent, $d6->doctype->name, $d6->doctype->internalSubset);
echo $d6->saveXML();
$d7 = new DOMDocument(); $d7->substituteEntities = true; $d7->loadXML('<!DOCTYPE r [<!ENTITY foo "bar">]><r>&foo;</r>'); var_dump($d7->documentElement->childNodes->length, $d7->documentElement->firstChild->nodeType); echo $d7->saveXML();
