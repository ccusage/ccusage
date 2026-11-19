#!/usr/bin/env nix
#! nix shell --inputs-from ../../.. nixpkgs#babashka-unwrapped nixpkgs#git nixpkgs#jq nixpkgs#diffutils --command bb

(ns diff-refs
  (:require [babashka.classpath :as cp]
            [babashka.fs :as fs]))

(cp/add-classpath (str (fs/parent (fs/absolutize *file*))))

(require '[diff-refs.main :as diff])

(when (= *file* (System/getProperty "babashka.file"))
  (System/exit
   (try
     (apply diff/diff-main *command-line-args*)
     (catch Exception error
       (binding [*out* *err*]
         (println (ex-message error)))
       2))))
