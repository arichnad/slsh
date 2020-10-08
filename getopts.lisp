#!/home/price/development/slsh/target/debug/sl-sh

(ns-import 'shell)
(ns-import 'test)
(ns-import 'iterator)

(defmacro debugln (&rest args)
    (if (nil? nil)
        `(println "=> " ,@args)))

(def 'sample "-la -c -b")

(def 'token-delim "-")

(def 'no-args "Getopts requires arguments.")

(def 'bad-first-arg "First argument must be a flag.")

(defn bad-option-arity (option expected)
    (str "Wrong number of arguments passed to " option ". Expected " expected
         " arguments."))

(defn is-single-char-arg (token)
    (and (= 2 (length token)) (str-starts-with token-delim token)))

(defn is-multi-char-arg (token)
    (str-starts-with (str token-delim token-delim) token))

(defn is-multi-single-char-args (token)
    (str-starts-with token-delim token))

(defn get-next-params
"this function looks through the vec-args and returns only those vec-args that
are meant to be the parameters to the command flag at idx, i.e.
the vec-args #(\"-a\" \"-b\" \"foo\" \"-c\")
-a has no intended params, because there are no string values after -a and
before the next token delimeted (-) option, in this case, -b, so if idx
was 0, get-next-params would return the empty vector. If the idx was 1,
get-next-params would return \"foo\" since that is the rest of the vector
up until the next token delimeted option, -c.
"
    (idx vec-args)
    (var 'possible-params (vec-slice vec-args (+ idx 1) (length vec-args)))
    ;; possible params is nil if at the end of a list, return an empty vec 
    ;; indicating no paramaters
    (when (nil? possible-params)
      (return-from get-next-params '#()))
    (var 'no-token-delim
         (str-split
           (str " " token-delim)
           (str-cat-list " " possible-params)))
    (var 'with-token-delim (str-split :whitespace (first no-token-delim)))
    ;; special case if this is no argument to a variable. with-token-delim
    ;; variable will be the rest of the string, must manually return an empty
    ;; vec indicating no parameters
    (if (str-starts-with token-delim (str-cat-list "" with-token-delim)) '#() with-token-delim))

(defn is-getopts-option-string (arg)
    (and (string? arg) (str-starts-with token-delim arg)))

(defn illegal-option (key)
    (str "Illegal option " key ", not in allowable arguments provided to getopts."))

(defn verify-arity (idx given-args options-map bindings-map)
    (var 'option (vec-nth idx given-args))
    (var 'key (to-symbol (str ":" option)))
    (var 'arity-map (hash-get options-map key))
    (var 'arity (if (nil? arity-map)
                  0
                  (hash-get arity-map :arity 0)))
    (when (nil? arity-map)
      (err (illegal-option option)))
    ;; in case we are at end of args vector but the last option expects more
    ;; params than rest of args vector has after idx.
    (when (>= (+ idx arity) (length given-args))
      (err (bad-option-arity option arity)))
    ;; since all options start with a " -" use a str-split/str-cat trick
    ;; to get a vector representing all args to current option
    (var 'potential-args (get-next-params idx given-args))
    (when (not (= (length potential-args) arity))
      (err (bad-option-arity option arity)))
    (hash-set! bindings-map key (if (empty-seq? potential-args) #t potential-args)))

(defn verify-all-options-valid (cmd-line-args options-map bindings-map)
    (var 'vec-args (collect-vec cmd-line-args))
    (debugln "vec-args: " vec-args)
    (for-i idx cmd in vec-args
         (do
           (debugln "cmd: " (vec-nth idx vec-args) ", idx: " idx)
           (cond
             ((is-multi-char-arg cmd) (verify-arity idx vec-args options-map bindings-map))
             ((is-single-char-arg cmd) (verify-arity idx vec-args options-map bindings-map))
             ((is-multi-single-char-args cmd)
                 (progn
                 ;; if the command in question looked like "-ab", de-multi-single-arged-str
                 ;; will be "-a -b" this way the new cmd-line-args list can
                 ;; be fed recursively to verify-all-options-valid
                 (var 'de-multi-single-arged-str
                      (str-split " " (str token-delim
                           (str-cat-list (str " " token-delim)
                           (collect-vec (str-replace (vec-nth idx vec-args) token-delim ""))))))
                 (var 'sub-vec (map str (append de-multi-single-arged-str (slice vec-args (+ 1 idx) (length vec-args)))))
                   (verify-all-options-valid sub-vec options-map bindings-map)
                   "a"))))))

(defn build-getopts-param (arity)
    (make-hash
        (list
        (join :arity arity))))

(defn valid-first-arg? (args)
    (when (not (is-getopts-option-string (first args)))
        (err bad-first-arg)))

(def 'nyi "not-yet-implemented")

(defn make-hash-with-keys (hmap)
    (make-hash (collect (map (fn (x) (join (to-symbol x) nil)) (hash-keys hmap)))))

;; TODO options-map this needs ability to set default values
;; TODO eliminate debugln or make it programmatically switchable and print
;;      useful info.
(defn getopts (options-map &rest args)
    (when (not (> (length args) 0))
        (err no-args))
    (valid-first-arg? args)
    (var 'bindings-map (make-hash-with-keys options-map))
    (verify-all-options-valid args options-map bindings-map)
    (debugln "bindings-map: " bindings-map)
    (err nyi))

(def 'test-options-map
    (make-hash
      (list
        (join :-l (build-getopts-param 0))
        (join :-m (build-getopts-param 0))
        (join :-a (build-getopts-param 1))
        (join :--c-arg (build-getopts-param 1))
        (join :--d-arg (build-getopts-param 2))
        (join :-b (build-getopts-param 3)))))

;;(println (expand-macro (with-keys (hash-keys test-options-map))))
;;(println (make-hash-with-keys test-options-map))

(assert-error-msg (getopts test-options-map) no-args)
(assert-error-msg (getopts test-options-map "a") bad-first-arg)
(assert-error-msg (getopts test-options-map "abc") bad-first-arg)

(assert-error-msg (getopts test-options-map "-a") (bad-option-arity "-a" 1))
(assert-error-msg (getopts test-options-map "--c-arg") (bad-option-arity "--c-arg" 1))
(assert-error-msg (getopts test-options-map "-a" "one-arg" "2-arg" "3") (bad-option-arity "-a" 1))
(assert-error-msg (getopts test-options-map "-l" "-a" "one-arg" "2-arg" "3") (bad-option-arity "-a" 1))
(assert-error-msg (getopts test-options-map "-b" "1" "2" "3" "-a") (bad-option-arity "-a" 1))
(assert-error-msg (getopts test-options-map "-b" "1" "3" "-a") (bad-option-arity "-b" 3))
(assert-error-msg (getopts test-options-map "-b" "1" "a" "-a" "2") (bad-option-arity "-b" 3))
(assert-error-msg (getopts test-options-map "-b" "1" "b") (bad-option-arity "-b" 3))
(assert-error-msg (getopts test-options-map "-b" "1" "2" "3" "4") (bad-option-arity "-b" 3))
(assert-error-msg (getopts test-options-map "-b" "1" "2" "3" "4" "--c-arg") (bad-option-arity "-b" 3))
(assert-error-msg (getopts test-options-map "-b" "1" "2" "3" "--c-arg") (bad-option-arity "--c-arg" 1))
(assert-error-msg (getopts test-options-map "-lma" "aaa" "-b" "1" "2" "3" "--c-arg" "1" "-d" "0") (illegal-option "-d"))
(assert-error-msg (getopts test-options-map "-lma" "aaa" "-b" "1" "2" "3" "--c-arg" "1" "-e") (illegal-option "-e"))

(assert-error-msg (getopts test-options-map "-ab" "an-argument") (bad-option-arity "-a" 1))
(assert-error-msg (getopts test-options-map "-lb" "an-argument") (bad-option-arity "-b" 3))

(assert-error-msg (getopts test-options-map "-lmb" "1" "2" "3") nyi)
(assert-error-msg (getopts test-options-map "-lmb" "1" "2" "3") nyi)
(assert-error-msg (getopts test-options-map "-b" "1" "2" "3" "-lm") nyi)
(assert-error-msg (getopts test-options-map "-a" "1") nyi)
(assert-error-msg (getopts test-options-map "-l" "-a" "one-arg") nyi)
(assert-error-msg (getopts test-options-map "-b" "1" "2" "3") nyi)
(assert-error-msg (getopts test-options-map "-b" "1" "2" "3" "-a" "1") nyi)
(assert-error-msg (getopts test-options-map "-lma" "aaa" "-b" "1" "2" "3" "--c-arg" "1") nyi)
(assert-error-msg (getopts test-options-map "-lma" "aaa" "-b" "1" "2" "3" "--c-arg" "1" "--d-arg" "1" "2") nyi)

;; (assert-error-msg (getopts test-options-map "-b" "an-argument") (bad-option-arity "-a" 1))

(def 'myit ((iterator::list-iter) :init '(1 2 3 4 5 6 7 8 9 10 11)))
(println (myit  :next!))
(def 'myslice  (myit  :slice 8))
(println (myslice  :next!))

(ns-pop) ;; must be last line
