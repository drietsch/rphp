<?php

// Global-namespace polyfill-style file: guards and definitions.

if (!function_exists('mb_strlen')) {
    function mb_strlen($s)
    {
        return \strlen($s);
    }
}
if (!\function_exists('iconv_strlen') && extension_loaded('mbstring')) {
    echo iconv_strlen('a');
}
if (class_exists('ArrayObject') && interface_exists(\Stringable::class) && !defined('PHP_WINDOWS_VERSION_MAJOR')) {
    $v = constant('PHP_VERSION') . \constant('E_ALL');
    define('SOME_FLAG', 1);
    define('E_STRICT', 2048);
}
if (function_exists("opcache_invalidate")) {
    opcache_invalidate(__FILE__, true);
}

interface Stringable
{
}
