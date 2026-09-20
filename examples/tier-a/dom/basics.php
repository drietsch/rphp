<?php
// DOMDocument basics: loading, the node properties, the serializer's
// escaping and `formatOutput` rule, factories, `importNode`, the
// `WRONG_DOCUMENT_ERR`.
chdir('/tmp'); // documentURI/baseURI print the working directory
$d = new DOMDocument();
var_dump($d->loadXML('<?xml version="1.0" encoding="UTF-8"?>
<root xmlns="urn:x" xmlns:p="urn:p" a="1" p:b="2"><!-- c --><child>t&amp;x</child><p:e><![CDATA[cd<]]></p:e><?pi data?>tail</root>'));
var_dump($d->documentElement === $d->documentElement, $d->documentElement->ownerDocument === $d);
$r = $d->documentElement;
var_dump($r->nodeName, $r->localName, $r->prefix, $r->namespaceURI, $r->nodeType, $r->nodeValue, $r->textContent, $r->childNodes->length, $r->attributes->length, $r->getAttribute('a'), $r->getAttribute('p:b'), $r->getAttributeNS('urn:p', 'b'), $r->hasAttribute('zz'), $r->getAttribute('zz'));
foreach ($r->childNodes as $i => $c) { echo $i, ": ", get_class($c), " ", $c->nodeType, " ", var_export($c->nodeName, true), " ", var_export($c->nodeValue, true), "\n"; }
echo $d->saveXML();
echo $d->saveXML($r), "\n";
$d->formatOutput = true;
echo $d->saveXML();
var_dump($d->xmlVersion, $d->xmlEncoding, $d->encoding, $d->xmlStandalone, $d->documentURI, $d->firstChild === $r, $r->firstChild->nodeType, $r->lastChild->nodeValue, $r->firstElementChild->tagName, $r->childElementCount);
$n = $d->createElement('new', 'val<>');
$r->appendChild($n);
$n->setAttribute('k', 'v"&');
$n->appendChild($d->createTextNode('<t>'));
$n->appendChild($d->createComment('cm'));
$n->appendChild($d->createCDATASection('cd'));
$n->appendChild($d->createProcessingInstruction('php', 'echo 1;'));
$d->formatOutput = false;
echo $d->saveXML(), "\n";
var_dump($n->getLineNo(), $r->getLineNo(), $n->parentNode === $r, $n->previousSibling->nodeValue, $n->nextSibling);
$x = new DOMXPath($d);
$x->registerNamespace('x', 'urn:x');
foreach ($x->query('//x:child') as $c) var_dump($c->textContent);
var_dump($x->evaluate('count(//x:child)'), $x->evaluate('string(/x:root/@a)'), $x->query('//nope')->length, $x->query('/x:root/*')->length, $x->evaluate('name(/*)'));
var_dump($d->getElementsByTagName('child')->length, $d->getElementsByTagName('*')->length, $d->getElementsByTagNameNS('urn:p', 'e')->item(0)->textContent);
$d2 = new DOMDocument('1.0', 'UTF-8');
$e = $d2->createElement('a'); $d2->appendChild($e); $e->setAttribute('x', '1'); $e->appendChild($d2->createElement('b'))->textContent = 'hi & bye';
echo $d2->saveXML(), $d2->saveXML($e), "\n";
var_dump($e->getAttributeNode('x')->value, $e->getAttributeNode('x')->nodeType, $e->attributes->getNamedItem('x')->nodeValue, $e->hasChildNodes(), $e->isSameNode($d2->documentElement), $e->C14N());
$imp = $d2->importNode($r->firstElementChild, true);
$d2->documentElement->appendChild($imp);
echo $d2->saveXML();
try { $d2->documentElement->appendChild($r); } catch (DOMException $ex) { echo get_class($ex), " ", $ex->getCode(), " ", $ex->getMessage(), "\n"; }
var_dump($e->removeChild($e->firstChild)->nodeName, $e->childNodes->length);
