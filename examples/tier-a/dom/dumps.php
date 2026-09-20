<?php
// var_dump of DOM objects: the computed properties in php's order, object
// values omitted.
chdir('/tmp'); // documentURI/baseURI print the working directory
$d = new DOMDocument; $d->loadXML("<r a=\"1\">t<!--c--><?p d?></r>"); foreach ([$d, $d->documentElement, $d->documentElement->firstChild, $d->documentElement->getAttributeNode("a"), $d->documentElement->childNodes[1], $d->documentElement->childNodes[2], $d->childNodes, $d->documentElement->attributes, $d->createDocumentFragment(), $d->implementation] as $o) { var_dump($o); }
