<?php
// ext/xmlreader: the attribute cursor, next() over subtrees, string and
// markup readers, expand() into a DOM node.
function t($f) {
    try {
        var_dump($f());
    } catch (\Throwable $e) {
        echo get_class($e), ": ", $e->getMessage(), "\n";
    }
}
$x = '<root xmlns:p="urn:p" a="1" p:b="2"><p:c k="v">text<d>deep</d><![CDATA[cd]]>tail</p:c><e/><f>x</f><g><h/></g></root>';
$r = XMLReader::fromString($x);
$r->read();
t(fn() => $r->getAttribute('a'));
t(fn() => $r->getAttribute('zz'));
t(fn() => $r->getAttribute('xmlns:p'));
t(fn() => $r->getAttribute('p:b'));
t(fn() => $r->getAttributeNo(0));
t(fn() => $r->getAttributeNo(2));
t(fn() => $r->getAttributeNo(9));
t(fn() => $r->getAttributeNo(-1));
t(fn() => $r->getAttributeNs('b', 'urn:p'));
t(fn() => $r->getAttributeNs('b', 'urn:x'));
t(fn() => $r->getAttributeNs('p', 'http://www.w3.org/2000/xmlns/'));
t(fn() => $r->lookupNamespace('p'));
t(fn() => $r->lookupNamespace('xml'));
t(fn() => $r->lookupNamespace('zz'));
t(fn() => $r->moveToAttribute('a'));
t(fn() => [$r->name, $r->value, $r->nodeType, $r->depth, $r->hasAttributes, $r->attributeCount]);
t(fn() => $r->moveToAttribute('nope'));
t(fn() => $r->name);
t(fn() => $r->moveToAttributeNo(0));
t(fn() => [$r->name, $r->value]);
t(fn() => $r->moveToAttributeNo(7));
t(fn() => $r->moveToAttributeNs('b', 'urn:p'));
t(fn() => [$r->name, $r->value, $r->prefix, $r->localName, $r->namespaceURI]);
t(fn() => $r->moveToFirstAttribute());
t(fn() => [$r->name, $r->value]);
t(fn() => $r->readString());
t(fn() => $r->moveToElement());
t(fn() => $r->moveToElement());
t(fn() => $r->readInnerXml());
t(fn() => $r->readOuterXml());
t(fn() => $r->readString());
$r->read();
t(fn() => [$r->name, $r->readInnerXml(), $r->readOuterXml(), $r->readString()]);
$r->read();
t(fn() => [$r->name, $r->nodeType, $r->readString(), $r->readInnerXml(), $r->readOuterXml()]);
t(fn() => $r->next());
t(fn() => [$r->name, $r->nodeType]);
t(fn() => $r->next());
t(fn() => [$r->name, $r->nodeType]);
t(fn() => $r->next());
t(fn() => [$r->name, $r->nodeType]);
t(fn() => $r->next('g'));
t(fn() => [$r->name, $r->nodeType]);
$n = $r->expand();
var_dump(get_class($n), $n->nodeName, $n->ownerDocument === null, $n->parentNode === null, $n->childNodes->length);
$d = new DOMDocument();
$d->appendChild($d->importNode($n, true));
echo $d->saveXML();
t(fn() => $r->next());
t(fn() => [$r->name, $r->nodeType, $r->getAttribute('a'), $r->hasAttributes, $r->attributeCount]);
t(fn() => $r->next());
t(fn() => [$r->name, $r->nodeType]);
t(fn() => $r->read());
t(fn() => $r->next());

echo "--- namespaces declared on copies\n";
$r = XMLReader::fromString('<root xmlns="urn:d" xmlns:p="urn:p"><p:a p:x="1"><b/><p:c/></p:a><q:z xmlns:q="urn:q"/></root>');
$r->read();
$r->read();
echo $r->readOuterXml(), "\n", $r->readInnerXml(), "\n";
$r->read();
echo $r->readOuterXml(), "\n";
$n = $r->expand();
var_dump(get_class($n), $n->namespaceURI);
$r->next();
$r->next();
echo $r->name, " ", $r->readOuterXml(), "\n";

echo "--- next() by name skips subtrees\n";
$r = XMLReader::fromString('<list><item><item>inner</item></item><other/><item>2</item></list>');
$r->read();
$r->read();
while ($r->name === 'item') {
    echo $r->readOuterXml(), "\n";
    if (!$r->next('item')) {
        break;
    }
}

echo "--- expand of text, before reading\n";
$r = XMLReader::fromString('<a>txt</a>');
t(fn() => $r->expand());
$r->read();
$r->read();
$n = $r->expand();
var_dump(get_class($n), $n->nodeValue);
