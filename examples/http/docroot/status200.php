<?php
// A status line naming 200 (Symfony's Response::sendHeaders() does this)
// earns no `Status:` under CGI; http_response_code() reports it.
header('HTTP/1.1 200 OK');
header('X-After: 1');
var_dump(http_response_code());
