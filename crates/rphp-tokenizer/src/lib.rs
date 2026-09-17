//! PHP-exact tokenizer: a direct implementation of the Zend scanner's state
//! machine producing lossless raw tokens with php-src token ids, backing
//! `token_get_all()` / `PhpToken::tokenize()` and `rphp --emit=tokens`.
#![forbid(unsafe_code)]
