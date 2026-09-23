<?php
// ext/xmlreader: the node sequence and each node's properties — doctype,
// elements and their END_ELEMENTs, <e/> without one, significant
// whitespace, entity references, comments, PIs, CDATA.
function walk(XMLReader $r) {
    while ($r->read()) {
        printf("%d %s local=%s prefix=%s ns=%s value=%s depth=%d empty=%d hasValue=%d hasAttrs=%d attrs=%d lang=%s\n",
            $r->nodeType, json_encode($r->name), $r->localName, $r->prefix, $r->namespaceURI, json_encode($r->value),
            $r->depth, $r->isEmptyElement, $r->hasValue, $r->hasAttributes, $r->attributeCount, $r->xmlLang);
        if ($r->nodeType == XMLReader::ELEMENT && $r->hasAttributes) {
            while ($r->moveToNextAttribute()) {
                printf("  attr %d %s=%s local=%s prefix=%s ns=%s depth=%d hasValue=%d\n", $r->nodeType, $r->name, $r->value,
                    $r->localName, $r->prefix, $r->namespaceURI, $r->depth, $r->hasValue);
            }
            $r->moveToElement();
        }
    }
    var_dump($r->nodeType, $r->name, $r->read());
}
$xml = "<?xml version='1.0'?>\n<!DOCTYPE root [<!ENTITY e 'ent'>]>\n"
    . "<root xmlns='urn:d' xmlns:p='urn:p' a='1' p:b='2' xml:lang='en'>\n"
    . "  <p:item id='x'>Hello &amp; &e; <b>bold</b></p:item>\n  <!-- c -->\n  <?pi data?>\n"
    . "  <![CDATA[ cd ]]>\n  <empty/>\n  <open></open>\n  <sp xml:space='preserve'>  </sp>\n"
    . "  <de xml:lang='de'><x>t</x></de>\n</root>\n";
$r = new XMLReader();
var_dump($r->XML($xml));
walk($r);

echo "--- substituted entities\n";
$r = XMLReader::fromString($xml);
var_dump($r->setParserProperty(XMLReader::SUBST_ENTITIES, true), $r->getParserProperty(XMLReader::SUBST_ENTITIES));
while ($r->read()) {
    echo $r->nodeType, ":", json_encode($r->value), " ";
}
echo "\n";
$r = XMLReader::fromString($xml, null, LIBXML_NOENT);
while ($r->read()) {
    echo $r->nodeType, " ";
}
echo "\n";

echo "--- blanks and CDATA options\n";
foreach ([0, LIBXML_NOBLANKS, LIBXML_NOCDATA] as $flags) {
    $r = XMLReader::fromString("<r>\n <a> </a>\n <b><![CDATA[x]]></b>\n</r>", null, $flags);
    while ($r->read()) {
        echo $r->nodeType, ":", json_encode($r->value), " ";
    }
    echo "\n";
}

echo "--- constants\n";
foreach ((new ReflectionClass('XMLReader'))->getConstants() as $k => $v) {
    echo "$k=$v\n";
}
var_dump(new XMLReader());
