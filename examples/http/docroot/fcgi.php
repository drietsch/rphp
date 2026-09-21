<?php
// The FastCGI SAPI's own surface: error_log() on the stderr stream,
// fastcgi_finish_request() ending the response early, getenv() over the
// request's parameters.
var_dump(PHP_SAPI, function_exists('fastcgi_finish_request'), getenv('REQUEST_METHOD'), getenv('NO_SUCH_PARAM'));
error_log('to the log');
echo "before finish\n";
if (function_exists('fastcgi_finish_request')) { var_dump(fastcgi_finish_request()); }
echo "after finish (dropped)\n";
error_log('after finish');
