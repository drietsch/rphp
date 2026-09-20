<?php
// php 8.4's Dom\HTMLDocument / Dom\XMLDocument: the HTML5 parser's tree
// (implied elements, foreign content, adoption agency), its serializer, the
// modern node classes and their properties, TokenList, innerHTML.
chdir('/tmp');
$d = Dom\HTMLDocument::createFromString('<!DOCTYPE html><body><p class="a b" id=x>Hi <b>there</b> &amp; you<br>x</p><!-- c --><?pi z?></body>');
var_dump(get_class($d), $d->nodeName, $d->nodeType, $d->doctype?->name, get_class($d->documentElement), $d->documentElement->nodeName, $d->documentElement->tagName, $d->body?->nodeName, $d->head?->nodeName, $d->title, $d->characterSet, $d->URL, $d->documentURI);
$p = $d->getElementsByTagName('p')->item(0);
var_dump(get_class($d->getElementsByTagName('p')), $d->getElementsByTagName('p')->length, $p->nodeName, $p->tagName, $p->localName, $p->namespaceURI, $p->id, $p->className, get_class($p->classList), $p->classList->length, $p->classList->contains('b'), $p->hasChildNodes(), $p->childNodes->length, get_class($p->childNodes), $p->children->length, get_class($p->attributes), $p->attributes->length);
foreach ($p->childNodes as $c) { echo get_class($c), " ", $c->nodeName, " ", var_export($c->nodeValue, true), " ", var_export($c->textContent, true), "\n"; }
foreach ($p->attributes as $a) { echo get_class($a), " ", $a->name, "=", $a->value, " ", $a->nodeName, "\n"; }
foreach ($p->attributes->getIterator() as $k => $a) { echo $k, ":", $a->name, "\n"; }
var_dump($p->innerHTML, $p->outerHTML, $p->textContent, $p->getAttribute('id'), $p->hasAttribute('class'), $p->getAttributeNames());
echo $d->saveHtml(), "\n---\n", $d->saveHtml($p), "\n---\n", $d->saveXml(), "\n";
$x = Dom\XMLDocument::createFromString('<r xmlns:p="u"><a p:b="1">t</a></r>');
var_dump(get_class($x), get_class($x->documentElement), $x->documentElement->nodeName, $x->documentElement->firstChild->nodeName, $x->xmlVersion, $x->xmlEncoding, $x->xmlStandalone, $x->formatOutput);
echo $x->saveXml(), "\n";
$e = $d->createElement('div'); $e->textContent = 'new'; $e->setAttribute('data-x', '1'); $p->appendChild($e); $p->append('tail'); echo $d->saveHtml($p), "\n";
var_dump($d->querySelector('p b')?->textContent, count($d->querySelectorAll('p *')), $p->closest('body')?->nodeName, $p->matches('p.a'));
$t = $d->createTextNode('t'); var_dump(get_class($t), $t instanceof Dom\CharacterData, $t instanceof Dom\Node, $p instanceof Dom\Element, $p instanceof Dom\HTMLElement, $d instanceof Dom\Document);
$e2 = Dom\HTMLDocument::createEmpty(); var_dump($e2->documentElement, $e2->body, $e2->characterSet); $e2->append($e2->createElement('html')); echo $e2->saveHtml(), "\n";
try { Dom\HTMLDocument::createFromString(''); } catch (Throwable $ex) { echo get_class($ex), ": ", $ex->getMessage(), "\n"; }
try { Dom\XMLDocument::createFromString('<bad'); } catch (Throwable $ex) { echo get_class($ex), ": ", $ex->getMessage(), "\n"; }
var_dump((string)Dom\AdjacentPosition::BeforeBegin->value);
$d3 = Dom\HTMLDocument::createFromString('<div>x</div>', LIBXML_NOERROR); echo $d3->saveHtml(), "\n"; var_dump($d3->body->firstChild->nodeName);
$d = new DOMDocument; $d->loadHTML('<textarea><b>x</b> &amp;</textarea><title>a <i>b</i> &amp;</title><pre>
z</pre><?pi z?>');
echo $d->saveHTML();
$d = Dom\HTMLDocument::createFromString('<!DOCTYPE html><textarea><b>x</b> &amp;</textarea><title>a <i>b</i> &amp;</title><plaintext>a<b>', LIBXML_NOERROR);
echo $d->saveHtml(), "\n";
$d = Dom\HTMLDocument::createFromString('<!DOCTYPE html><table><td>1<tr><td>2</table><ul><li>a<li>b</ul><dl><dt>x<dd>y</dl><select><option>1<option>2</select><p>a<div>b</div>c</p>d<br/><a href=x>l<a>m</a>', LIBXML_NOERROR);
echo $d->saveHtml(), "\n";
$d = Dom\HTMLDocument::createFromString('<!DOCTYPE html><html lang=en><head><meta charset=utf-8><title>T</title></head><body class=b><h1>H</h1><noscript><p>n</p></noscript><iframe>x<b></iframe><svg><foreignObject><div>d</div></foreignObject><linearGradient/></svg><math><mtext><b>b</b></mtext></math><p>after', LIBXML_NOERROR);
echo $d->saveHtml(), "\n";
var_dump($d->getElementsByTagName('div')->item(0)->namespaceURI, get_class($d->getElementsByTagName('foreignObject')->item(0)), $d->getElementsByTagName('foreignObject')->item(0)->nodeName, $d->getElementsByTagName('b')->item(0)->namespaceURI, $d->getElementsByTagName('svg')->item(0)->tagName);
$d = Dom\HTMLDocument::createFromString('<!DOCTYPE html><body><p>a</body></html>trail<p>x', LIBXML_NOERROR);
echo $d->saveHtml(), "\n";
$d = Dom\HTMLDocument::createFromString("<!DOCTYPE html>\n<html>\n<head>\n<title>T</title>\n</head>\n<body>\n<p>a</p>\n</body>\n</html>\n", LIBXML_NOERROR);
echo $d->saveHtml(), "|\n";
var_dump($d->body->childNodes->length, $d->head->childNodes->length, $d->documentElement->childNodes->length);
$d = Dom\HTMLDocument::createFromString("<!DOCTYPE html><body><p>x<!--c--><b>y<i>z</b>w</i></p><br></br><img src='a'/><div/>after", LIBXML_NOERROR);
echo $d->saveHtml(), "|\n";
$d = new DOMDocument; $d->loadHTML('<!DOCTYPE HTML><p>a &amp b &foo; &copy=1 &#65; &#x42; &#128; &nbsp;x</p>');
var_dump($d->doctype->name, $d->getElementsByTagName('p')->item(0)->textContent); echo $d->saveHTML();
$d = Dom\HTMLDocument::createFromString('<!DOCTYPE HTML><p>a &amp b &foo; &copy=1 &#65; &#x42; &#128; &nbsp;x</p><a href="?a=1&copy=2&amp;c=3&b=4">l</a>');
var_dump($d->doctype->name, $d->getElementsByTagName('p')->item(0)->textContent, $d->getElementsByTagName('a')->item(0)->getAttribute('href')); echo $d->saveHtml();
$x2 = Dom\XMLDocument::createFromString('<r xmlns="urn:u" xmlns:p="urn:v"><p:a><b/><p:c/></p:a></r>');
$a = $x2->documentElement->firstChild;
echo $x2->saveXml($a), "\n", $a->outerHTML, "\n", $a->innerHTML, "\n";
$l = new DOMDocument; $l->loadXML('<r xmlns="urn:u" xmlns:p="urn:v"><p:a><b/><p:c/></p:a></r>');
echo $l->saveXML($l->documentElement->firstChild), "\n";
$h = Dom\HTMLDocument::createFromString('<!DOCTYPE html><p>x<br>y</p>');
echo $h->saveXml($h->body), "\n", $h->body->outerHTML, "\n";
