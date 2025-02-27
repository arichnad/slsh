(defmacro setq (sym bind)
	`(set (quote ,sym) ,bind))

(defmacro defq (sym bind)
	`(def (quote ,sym) ,bind))

(defmacro defn (name args body)
	`(defq ,name (fn ,args ,body)))

(defmacro loop (params bindings body)
		`((fn ,params ,body) ,@bindings))

(defmacro dotimes (times body)
	(let ((idx-name (gensym)))
	`(loop (idx-name) (,times) (progn
		(eval ,body)
		(if (> idx-name 1) (recur (- idx-name 1)))))))

(defmacro dotimesi (idx-bind times body)
	(let ((stop-name (gensym)))
	`(loop (,idx-bind stop-name) (0 (- ,times 1)) (progn
		(eval ,body)
		(if (< ,idx-bind stop-name) (recur (+ ,idx-bind 1) stop-name))))))

(defmacro for (bind in_list body)
	`(let ((,bind))
		(if (> (length ,in_list) 0)
			(loop (plist) (,in_list) (progn
				(setq ,bind (first plist))
				(eval ,body)
				(if (> (length plist) 1) (recur (rest plist))))))))

(defmacro fori (idx_bind bind in_list body)
	`(let ((,bind)(,idx_bind))
		(if (> (length ,in_list) 0)
			(loop (plist idx) (,in_list 0) (progn
				(setq ,bind (first plist))
				(setq ,idx_bind idx)
				(eval ,body)
				(if (> (length plist) 1) (recur (rest plist) (+ idx 1))))))))

(defmacro match (condition &rest branches)
	(let ((cond-name) (out_list '()) (make-cond))
		(setq make-cond (fn (condition val action others)
			(if (null val) action
				(if (null others) `(if (= ,condition ,val) ,action)
					`(if (= ,condition ,val) ,action ,(make-cond condition (first (first others)) (nth 1 (first others)) (rest others)))))))
		(setq cond-name condition)
		(make-cond cond-name (first (first branches)) (nth 1 (first branches)) (rest branches))))
