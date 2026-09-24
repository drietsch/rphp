<?php
// ResourceBundle::getLocales() / resourcebundle_locales(): the locales of
// ICU's main tree and of its rule-based number format tree.
$all = ResourceBundle::getLocales('');
var_dump(count($all), array_slice($all, 0, 5), in_array('en_US_POSIX', $all, true), in_array('de_AT', $all, true));
var_dump(count(resourcebundle_locales('')) === count($all));
var_dump(ResourceBundle::getLocales('ICUDATA-rbnf'));
var_dump(ResourceBundle::getLocales('no-such-bundle'), intl_get_error_code(), intl_get_error_message());
var_dump(class_exists('ResourceBundle'), (new ReflectionClass('ResourceBundle'))->implementsInterface('Countable'));
