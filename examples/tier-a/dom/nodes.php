<?php
// DOMNode / DOMElement: properties, readonly refusals, `nodeValue` and
// `textContent` writes, namespace lookups, node paths, the DOMException
// codes, orphan nodes from `new DOMElement`, fragments, node lists,
// `registerNodeClass`, character data, `createElementNS`/`setAttributeNS`.
chdir('/tmp'); // documentURI/baseURI print the working directory
$d = new DOMDocument; $d->loadXML('<r xmlns:p="urn:p"><a p:x="1">t</a><!--c--></r>');
$r = $d->documentElement; $a = $r->firstChild;
var_dump($r->nodeValue, $r->parentNode === $d, $d->parentNode, $d->ownerDocument, $r->ownerDocument === $d, $d->nodeValue, $d->textContent, $d->nodeType, $d->nodeName, $r->isConnected, $d->createElement('x')->isConnected, $r->baseURI, $a->prefix, $a->namespaceURI, $r->attributes, $a->attributes->length, $d->attributes, $a->firstChild->attributes, $r->lastChild->parentElement === $r, $d->documentElement->parentElement);
try { $r->nodeName = 'x'; } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
try { $r->nodeType = 2; } catch (Error $e) { echo get_class($e), ": ", $e->getMessage(), "\n"; }
$r->nodeValue = 'replaced'; var_dump($r->childNodes->length, $r->textContent); echo $d->saveXML($r), "\n";
$r->textContent = 'again<'; echo $d->saveXML($r), "\n";
$a->nodeValue = 'av'; var_dump($a->nodeValue);
var_dump($r->nope);
$r->dyn = 5; var_dump($r->dyn, isset($r->nodeName), isset($r->nope), isset($r->nodeValue), empty($r->childNodes));
var_dump($r->lookupNamespaceURI('p'), $r->lookupNamespaceURI(null), $r->lookupPrefix('urn:p'), $r->isDefaultNamespace('urn:p'), $r->hasAttributes(), $r->getNodePath(), $r->firstChild->getNodePath(), $d->getNodePath());
var_dump($r->cloneNode()->childNodes->length, $r->cloneNode(true)->childNodes->length, $r->isSameNode($r), $r->isSameNode($r->cloneNode()), $r->isEqualNode($r->cloneNode(true)));
$t = $d->createTextNode('x');
var_dump($t->ownerDocument === $d, $t->parentNode, $t->getLineNo());
try { $d->appendChild($t); } catch (DOMException $e) { echo $e->getCode(), " ", $e->getMessage(), "\n"; }
try { $r->insertBefore($t, $d->createElement('z')); } catch (DOMException $e) { echo $e->getCode(), " ", $e->getMessage(), "\n"; }
try { $r->removeChild($d->createElement('z')); } catch (DOMException $e) { echo $e->getCode(), " ", $e->getMessage(), "\n"; }
try { $t->appendChild($d->createElement('z')); } catch (DOMException $e) { echo $e->getCode(), " ", $e->getMessage(), "\n"; }
try { $r->appendChild($r); } catch (DOMException $e) { echo $e->getCode(), " ", $e->getMessage(), "\n"; }
$e2 = new DOMElement('n', 'v'); var_dump($e2->ownerDocument, $e2->nodeValue, $e2->textContent);
try { $e2->setAttribute('a', 'b'); } catch (DOMException $e) { echo $e->getCode(), " ", $e->getMessage(), "\n"; }
$r->appendChild($e2); var_dump($e2->ownerDocument === $d, $e2->parentNode === $r); echo $d->saveXML($r), "\n";
$f = $d->createDocumentFragment(); $f->appendChild($d->createElement('f1')); $f->appendXML('<f2 a="1">x</f2>tail'); var_dump($f->childNodes->length); $r->appendChild($f); var_dump($f->childNodes->length); echo $d->saveXML($r), "\n";
var_dump(count($r->childNodes), iterator_to_array($r->childNodes) === iterator_to_array($r->childNodes), $r->childNodes->item(0)->nodeName, $r->childNodes->item(99), $r->childNodes[1]->nodeName);
class MyEl extends DOMElement { function hi() { return 'hi ' . $this->tagName; } }
$d->registerNodeClass('DOMElement', 'MyEl');
var_dump(get_class($d->documentElement), get_class($d->createElement('q')), $d->createElement('q')->hi(), get_class($d->createElement('q')->appendChild($d->createElement('w'))));
var_dump($r->firstChild->wholeText ?? null, $d->doctype, $d->implementation instanceof DOMImplementation, $d->implementation->hasFeature('Core', '2.0'));
$c = $d->createComment('cc'); var_dump($c->data, $c->length, $c->nodeValue); $c->data = 'dd'; var_dump($c->nodeValue); $c->appendData('e'); var_dump($c->data, $c->substringData(1, 2));
$tx = $d->createTextNode('hello world'); $r->appendChild($tx); $sp = $tx->splitText(5); var_dump($tx->data, $sp->data, $sp->previousSibling === $tx);
var_dump($d->createElement('a')->hasChildNodes(), $d->createElement('a', 'x')->hasChildNodes());
try { $d->createElement('1bad'); } catch (DOMException $e) { echo $e->getCode(), " ", $e->getMessage(), "\n"; }
try { new DOMElement(''); } catch (Throwable $e) { echo get_class($e), " ", $e->getCode(), " ", $e->getMessage(), "\n"; }
var_dump($d->getElementById('nope'));
$d->documentElement->setAttribute('id', 'me'); var_dump($d->getElementById('me') === $d->documentElement);
$x = $d->createElementNS('urn:q', 'q:el'); $r->appendChild($x); var_dump($x->prefix, $x->localName, $x->namespaceURI, $x->tagName); echo $d->saveXML($r), "\n";
$y = $d->createElementNS('urn:y', 'el2'); $r->appendChild($y); echo $d->saveXML($r), "\n";
$y->setAttributeNS('urn:q', 'q:at', 'v'); $y->setAttributeNS('http://www.w3.org/2000/xmlns/', 'xmlns:z', 'urn:z'); echo $d->saveXML($y), "\n"; var_dump($y->getAttributeNS('urn:q', 'at'), $y->hasAttributeNS('urn:q', 'at'), $y->attributes->length);
