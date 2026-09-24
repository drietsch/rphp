<?php
// intl.use_exceptions with MessageFormatter and IntlListFormatter, and the
// flow of Symfony's IntlFormatter: construct, format, report the error.
function symfony_format(string $locale, string $message, array $parameters): string {
    try {
        $formatter = new MessageFormatter($locale, $message);
    } catch (IntlException $e) {
        return sprintf('Invalid message format (error #%d): ', intl_get_error_code()) . intl_get_error_message();
    }
    foreach ($parameters as $key => $value) {
        if (in_array($key[0] ?? null, ['%', '{'], true)) {
            unset($parameters[$key]);
            $parameters[trim($key, '%{ }')] = $value;
        }
    }
    if (false === $out = $formatter->format($parameters)) {
        return sprintf('Unable to format message (error #%s): ', $formatter->getErrorCode()) . $formatter->getErrorMessage();
    }
    return $out;
}
echo symfony_format('en', 'There {apples, plural, =0 {are no apples} one {is one apple} other {are # apples}}', ['%apples%' => 3]), "\n";
echo symfony_format('fr', '{gender, select, female {Elle} other {Il}} a {n, plural, one {# pomme} other {# pommes}}', ['{gender}' => 'female', 'n' => 2]), "\n";
echo symfony_format('en', 'Broken {apples, plural, one {x}}', []), "\n";
echo symfony_format('en', '{a} {a,number}', ['a' => 1]), "\n";

ini_set('intl.use_exceptions', '1');
try {
    msgfmt_create('en', '{0');
} catch (IntlException $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}
try {
    MessageFormatter::formatMessage('en', '{0,number,integer}', [2147483648]);
} catch (IntlException $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}
try {
    (new IntlListFormatter('en'))->format(["\xff"]);
} catch (IntlException $e) {
    echo get_class($e), ': ', $e->getMessage(), "\n";
}
ini_set('intl.use_exceptions', '0');
var_dump(msgfmt_create('en', '{0'), intl_get_error_code());
