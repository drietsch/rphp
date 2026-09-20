<?php
// XPath 1.0 through DOMXPath: axes, node tests, predicates, the core
// function library, namespaces, error reporting, `php:function`.
$d = new DOMDocument; $d->loadXML('<?xml version="1.0"?>
<root xmlns:p="urn:p" id="r"><a n="1">one</a><a n="2">two<b>deep</b></a><p:c>three</p:c><!-- cm --><?pi x?>  <a n="3"/><d xmlns="urn:d"><e>in default</e></d></root>');
$x = new DOMXPath($d);
$x->registerNamespace('p', 'urn:p'); $x->registerNamespace('dd', 'urn:d');
function q($x, $e, $ctx = null) { $r = $x->evaluate($e, $ctx); if ($r instanceof DOMNodeList) { $o = []; foreach ($r as $n) { $o[] = $n->nodeName . (($n->nodeType == 2 || $n->nodeType == 3 || $n->nodeType == 8 || $n->nodeType == 7) ? '=' . $n->nodeValue : ''); } echo $e, " => [", implode(', ', $o), "]\n"; } else { echo $e, " => ", var_export($r, true), " (", gettype($r), ")\n"; } }
foreach (['/root/a', '//a', '//a[2]', '//a[last()]', '//a[@n="3"]', '//a[@n>1]', 'count(//a)', 'string(//a)', '//a/text()', '//p:c', 'name(//p:c)', 'local-name(//p:c)', 'namespace-uri(//p:c)', '//dd:e', '//e', '/root/*', '/root/node()', '//comment()', '//processing-instruction()', '//processing-instruction("pi")', '//@n', '//a/@n', 'sum(//@n)', 'concat("a", "b", 1)', 'contains("hello", "ell")', 'starts-with("hello", "he")', 'substring("hello", 2, 3)', 'substring("hello", 2)', 'substring-before("a-b", "-")', 'substring-after("a-b", "-")', 'string-length("héllo")', 'normalize-space("  a   b ")', 'translate("abc", "ab", "AB")', 'not(true())', 'boolean(//zzz)', 'number("3.5") + 1', '10 div 4', '10 mod 3', '-3', '1 = 1', '"1" = 1', '//a[position() < 3]', '//a[b]', '//a[not(b)]', '//b/..', '//b/ancestor::*', '//b/ancestor-or-self::*', '//a[1]/following-sibling::*', '//a[3]/preceding-sibling::a', '//b/preceding::*', '//a[1]/following::a', '//a[2]/descendant-or-self::node()', '//root/child::a[1]', '(//a)[2]', '//a | //b', '//a[b] | //p:c', 'string(/root/@id)', '/root/@id', '//*[@id]', 'id("r")', 'floor(1.5)', 'ceiling(1.2)', 'round(2.5)', 'round(-2.5)', 'string(1.0)', 'string(1.5)', 'string(1 div 0)', 'string(-1 div 0)', 'string(0 div 0)', 'string(true())', '//a[.="two"]', '//a[contains(., "o")]', 'count(//a[string-length(@n) = 1])', 'lang("en")', '//text()[normalize-space()]', 'name()', 'name(/)', '/', 'self::node()', '..', '@*', '//d/*', '//a[2]//text()', '//*[self::a or self::b]', 'position()', 'last()', "//a[@n='1' or @n='2']", '//a[@n!="1"]', '1 < 2', '2 >= 2', 'string(//a[1]/@n) + 1', '//a[3] and //b', 'count(//node())', 'count(/root/text())', '//root/comment()/..', 'boolean(0)', 'boolean("")', 'boolean("0")', 'number("abc")', 'number(true())', 'number(//a[1]/@n)', 'string(//a[2])', 'string(//zz)', '//a[@n]/@n', 'local-name(//dd:e/..)', 'namespace-uri(//e)'] as $e) { q($x, $e); }
q($x, 'a', $d->documentElement);
q($x, '.', $d->documentElement->firstChild);
q($x, 'ancestor::*', $d->getElementsByTagName('b')->item(0));
var_dump($x->query('//a')->length, $x->query('//a', $d->documentElement)->length, $x->query('a', $d->documentElement)->length);
$x2 = new DOMXPath($d); var_dump($x2->query('//dd:e'), libxml_get_last_error());
var_dump($x2->query('//p:c')->length);
var_dump($x2->evaluate('bad('), $x2->query('//a[')); 
var_dump($x->document === $d, $x->registerNodeNamespaces);
$x->registerNamespace('php', 'http://php.net/xpath'); $x->registerPhpFunctions();
q($x, 'php:function("strtoupper", string(//a[1]))');
q($x, 'php:functionString("strlen", //a[1])');
q($x, '//a[php:function("intval", string(@n)) > 1]');
$x->registerPhpFunctions(['strrev']);
q($x, 'php:function("strrev", "abc")');
q($x, 'php:function("strtolower", "ABC")');
