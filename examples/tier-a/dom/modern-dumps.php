<?php
// The modern DOM's dumps, class surface and diagnostics: property tables
// per class, LibXMLError entries the HTML5 parser records, DTD maps,
// createElement's casing, Implementation factories, namespaces.
chdir('/tmp');
$d = Dom\HTMLDocument::createFromString('<!DOCTYPE html><html><head><title>T</title></head><body><p class="a" id="x">Hi<!--c--><?pi z?></p></body></html>', LIBXML_NOERROR);
$p = $d->getElementsByTagName('p')->item(0);
var_dump($d, $p, $p->firstChild, $p->attributes->item(0), $d->doctype, $p->childNodes->item(1), $p->classList, $p->childNodes, $d->children, $p->attributes);
$x = Dom\XMLDocument::createFromString('<?xml version="1.0"?><r><![CDATA[c]]><?pi d?></r>');
var_dump($x, $x->documentElement, $x->documentElement->firstChild, $x->documentElement->lastChild, $x->createDocumentFragment(), $x->implementation);
$d = Dom\HTMLDocument::createFromString('<!DOCTYPE html><p id=a>1</p><p>2</p>');
$l = $d->getElementsByTagName('p');
var_dump($l[1]->textContent, isset($l[5]), $l->namedItem('a')?->textContent, $d->childNodes[1]->nodeName, $d->body->attributes['x'] ?? 'none', $d->body->firstChild->attributes['id']->value);
$x = Dom\XMLDocument::createFromString('<!DOCTYPE r [<!ENTITY e "v"><!NOTATION n SYSTEM "s">]><r>&e;</r>');
var_dump(get_class($x->doctype->entities), $x->doctype->entities->length, $x->doctype->entities['e']?->nodeName, get_class($x->doctype->notations), $x->documentElement->firstChild->nodeName, get_class($x->documentElement->firstChild));
var_dump($x->documentElement->prefix, $x->createAttribute('q')->prefix, $x->createElementNS('u','a:b')->prefix);
var_dump(Dom\XMLDocument::createEmpty()->documentURI, Dom\HTMLDocument::createEmpty()->documentURI, Dom\XMLDocument::createFromString('<a/>')->baseURI);
echo Dom\XMLDocument::createEmpty()->saveXml(), "|\n";
$e = Dom\XMLDocument::createEmpty(); $e->append($e->createElement('r')); echo $e->saveXml(), "|\n"; var_dump($e->xmlStandalone);
libxml_use_internal_errors(true);
$d = new DOMDocument; $d->loadHTML('<p>a &amp b</p>');
foreach (libxml_get_errors() as $e) echo "L: {$e->level} {$e->code} {$e->line}:{$e->column} {$e->message}";
libxml_clear_errors();
$d = Dom\HTMLDocument::createFromString("<!DOCTYPE html SYSTEM 'x'>");
$d = Dom\HTMLDocument::createFromString("  <!DOCTYPE html PUBLIC 'x' 'y'>");
$d = Dom\HTMLDocument::createFromString("\n <!DOCTYPE foo>");
$d = Dom\HTMLDocument::createFromString("<!DOCTYPE html><p>&amp b &#128; &#0; &#xD800; &notanentity; &copy</p>");
$d = Dom\HTMLDocument::createFromString("<!DOCTYPE html><p>x</p></p></div><b>a<p>b</b>");
$d = Dom\HTMLDocument::createFromString("<!DOCTYPE html><p>x</p><!DOCTYPE html>");
foreach (libxml_get_errors() as $e) echo "M: {$e->level} {$e->code} {$e->line}:{$e->column} {$e->message}\n";
libxml_clear_errors();
$x = Dom\XMLDocument::createFromString('<r xmlns="u"/>');
foreach (libxml_get_errors() as $e) echo "X: {$e->level} {$e->code} {$e->line}:{$e->column} {$e->message}";
libxml_use_internal_errors(false);
echo $d->saveHtml(), "\n";
libxml_use_internal_errors(true);
foreach (['<p>a & b</p>', '<a href="?a=1&b=2">l</a>', '<p>&#xZ; &# x &#65 y</p>', "<p>a &amp\nb &copy c</p>", '<p>&unknown;</p>', '<p>x</p></div>', '<p>&nbsp&nbsp;</p>'] as $src) {
  $d = new DOMDocument; $d->loadHTML($src);
  echo $src, " => ", $d->saveHTML($d->getElementsByTagName('body')->item(0)), "\n";
  foreach (libxml_get_errors() as $e) echo "  L: {$e->level} {$e->code} {$e->line}:{$e->column} {$e->message}";
  libxml_clear_errors();
}
libxml_use_internal_errors(true);
$d = Dom\HTMLDocument::createFromString("<p>x</p>\n<?pi?><b>y</i>");
var_dump(libxml_get_errors());
libxml_use_internal_errors(false);
echo $d->saveHtml(), "\n";
$d = Dom\HTMLDocument::createFromString('<p>x &nbsp; "q" <a href="?a=1&b=2">l</a>é</p><script>if (a < b) {}</script><textarea>&lt;x&gt;</textarea><pre>
z</pre>');
echo $d->saveHtml(), "\n";
echo $d->saveXml(), "\n";
var_dump($d->body->firstChild->firstChild->data);
$d = Dom\HTMLDocument::createFromString('<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.0 Strict//EN" "http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd"><html><body><P CLASS=Z><svg viewBox="1"><circle/></svg><MATH><mi>x</mi></MATH><input disabled value=""><img src=a.png alt=A><table><tr><td>1<td>2</table></body></html>');
echo $d->saveHtml(), "\n";
foreach ($d->getElementsByTagName('p')->item(0)->childNodes as $c) echo get_class($c), ' ', $c->nodeName, ' ', $c->localName, ' ', $c->namespaceURI, "\n";
var_dump($d->doctype->publicId, $d->doctype->systemId);
$e = $d->createElement('DIV'); var_dump($e->nodeName, $e->tagName, $e->localName, $e->namespaceURI, get_class($e));
$e = $d->createElementNS('urn:x', 'p:Foo'); var_dump($e->nodeName, $e->tagName, get_class($e));
$e = $d->createElementNS('http://www.w3.org/1999/xhtml', 'Foo'); var_dump($e->nodeName, $e->tagName, get_class($e));
$x = Dom\XMLDocument::createFromString('<r><a/></r>'); $e = $x->createElement('DIV'); var_dump($e->nodeName, get_class($e), $x->documentURI);
$e->innerHTML = '<b>1</b>t'; echo $x->saveXml($e), "\n";
$h = $d->createElement('div'); $h->innerHTML = '<p>1<p>2'; echo $d->saveHtml($h), "\n"; $h->innerHTML = ''; echo $d->saveHtml($h), "|\n";
$d->body->insertAdjacentHTML(Dom\AdjacentPosition::AfterBegin, '<i>first</i>'); echo $d->saveHtml($d->body), "\n";
var_dump($d->body->getElementsByClassName('Z')->length, $d->getElementsByClassName('Z')->item(0)->tagName);
try { $x->createElement('1bad'); } catch (DOMException $e) { echo $e->getMessage(), ' ', $e->getCode(), "\n"; }
try { $d->createElement('1bad'); } catch (DOMException $e) { echo $e->getMessage(), ' ', $e->getCode(), "\n"; }
$dt = $d->implementation->createDocumentType('html', '', ''); var_dump(get_class($dt));
$nd = $d->implementation->createHTMLDocument('Ttl'); echo $nd->saveHtml(), "\n";
$nd = $d->implementation->createDocument('urn:a', 'a:root'); echo $nd->saveXml(), "\n"; var_dump(get_class($nd));
var_dump($d->body->classList->value, $d->body->id, $d->body->className);
$p = $d->getElementsByTagName('p')->item(0);
$p->classList->add('q', 'r'); $p->classList->remove('Z'); var_dump($p->classList->value, $p->classList->toggle('q'), $p->classList->toggle('q'), $p->classList->replace('r', 's'), $p->getAttribute('class'), count($p->classList), iterator_to_array($p->classList));
try { $p->classList->add(''); } catch (DOMException $e) { echo get_class($e), ': ', $e->getMessage(), ' ', $e->getCode(), "\n"; }
try { $p->classList->add('a b'); } catch (DOMException $e) { echo get_class($e), ': ', $e->getMessage(), ' ', $e->getCode(), "\n"; }
$d->title = 'New'; echo $d->saveHtml($d->head), "\n"; var_dump($d->title);
var_dump($d->body->getRootNode() === $d, $d->body->contains($p), $p->compareDocumentPosition($d->body));
