package main

import (
	"archive/tar"
	"bytes"
	"compress/gzip"
	"crypto/sha256"
	"crypto/tls"
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"os"
	"strings"
	"time"
)

const (
	registryName = "registry.marsa-proxy.invalid"
	registryAddr = "127.0.0.1:5443"
	proxyAddr    = "127.0.0.1:43128"
)

type artifact struct {
	mediaType string
	data      []byte
}

func (a artifact) digest() string {
	return fmt.Sprintf("sha256:%x", sha256.Sum256(a.data))
}

func (a artifact) descriptor() map[string]any {
	return map[string]any{"mediaType": a.mediaType, "digest": a.digest(), "size": len(a.data)}
}

type repository struct {
	manifest artifact
	blobs    map[string]artifact
}

func image(marker string) repository {
	var raw bytes.Buffer
	tw := tar.NewWriter(&raw)
	if err := tw.WriteHeader(&tar.Header{Name: "fixture-marker", Size: int64(len(marker)), Mode: 0o644, Typeflag: tar.TypeReg}); err != nil {
		panic("could not write fixture tar header")
	}
	if _, err := tw.Write([]byte(marker)); err != nil {
		panic("could not write fixture layer")
	}
	if err := tw.Close(); err != nil {
		panic("could not close fixture tar")
	}

	var compressed bytes.Buffer
	gz := gzip.NewWriter(&compressed)
	if _, err := gz.Write(raw.Bytes()); err != nil {
		panic("could not compress fixture layer")
	}
	if err := gz.Close(); err != nil {
		panic("could not close fixture layer")
	}
	layer := artifact{"application/vnd.oci.image.layer.v1.tar+gzip", compressed.Bytes()}
	config := jsonArtifact("application/vnd.oci.image.config.v1+json", map[string]any{
		"architecture": "arm64",
		"os":           "linux",
		"config":       map[string]any{},
		"rootfs":       map[string]any{"type": "layers", "diff_ids": []string{fmt.Sprintf("sha256:%x", sha256.Sum256(raw.Bytes()))}},
	})
	manifest := jsonArtifact("application/vnd.oci.image.manifest.v1+json", map[string]any{
		"schemaVersion": 2,
		"mediaType":     "application/vnd.oci.image.manifest.v1+json",
		"config":        config.descriptor(),
		"layers":        []map[string]any{layer.descriptor()},
	})
	return repository{manifest, map[string]artifact{config.digest(): config, layer.digest(): layer}}
}

func jsonArtifact(mediaType string, value map[string]any) artifact {
	b, err := json.Marshal(value)
	if err != nil {
		panic("could not serialize fixture image")
	}
	return artifact{mediaType, b}
}

func registry(w http.ResponseWriter, r *http.Request) {
	if r.Host != registryName || (r.Method != http.MethodGet && r.Method != http.MethodHead) {
		http.NotFound(w, r)
		return
	}
	w.Header().Set("Docker-Distribution-API-Version", "registry/2.0")
	if r.URL.Path == "/v2/" {
		w.WriteHeader(http.StatusOK)
		return
	}
	parts := strings.Split(strings.TrimPrefix(r.URL.Path, "/"), "/")
	if len(parts) != 5 || parts[0] != "v2" || parts[1] != "marsa" {
		http.NotFound(w, r)
		return
	}
	repo, ok := images[parts[2]]
	if !ok {
		http.NotFound(w, r)
		return
	}
	var a artifact
	switch parts[3] {
	case "manifests":
		if parts[4] == "proof" || parts[4] == repo.manifest.digest() {
			a = repo.manifest
			log.Printf("m4-21 fixture %s manifest", parts[2])
		}
	case "blobs":
		a = repo.blobs[parts[4]]
	}
	if a.data == nil {
		http.NotFound(w, r)
		return
	}
	w.Header().Set("Content-Type", a.mediaType)
	w.Header().Set("Docker-Content-Digest", a.digest())
	http.ServeContent(w, r, "", time.Time{}, bytes.NewReader(a.data))
}

func proxy(w http.ResponseWriter, r *http.Request) {
	relay(w, r, registryAddr, nil)
}

func relay(w http.ResponseWriter, r *http.Request, origin string, interceptor *tls.Config) {
	// The fixture grants just one owned destination on guest loopback.
	if r.Method != http.MethodConnect || r.Host != registryName+":443" {
		http.Error(w, "fixture destination denied", http.StatusForbidden)
		return
	}
	upstream, err := net.DialTimeout("tcp4", origin, 2*time.Second)
	if err != nil {
		http.Error(w, "fixture registry unavailable", http.StatusBadGateway)
		return
	}
	defer upstream.Close()

	client, buffer, err := http.NewResponseController(w).Hijack()
	if err != nil {
		return
	}
	defer client.Close()
	deadline := time.Now().Add(25 * time.Second)
	client.SetDeadline(deadline)
	upstream.SetDeadline(deadline)
	if _, err := buffer.WriteString("HTTP/1.1 200 Connection Established\r\n\r\n"); err != nil {
		return
	}
	if err := buffer.Flush(); err != nil {
		return
	}
	var reader io.Reader = buffer.Reader
	if interceptor != nil {
		// Docker must authenticate the synthetic registry name on this connection.
		// The backend is a separate HTTP-only fixture on guest loopback.
		secure := tls.Server(client, interceptor)
		if err := secure.Handshake(); err != nil {
			return
		}
		client, reader = secure, secure
		log.Print("m4-11 fixture intercepted registry TLS")
	} else {
		log.Print("m4-21 fixture accepted restricted CONNECT")
	}
	done := make(chan struct{})
	go func() {
		defer close(done)
		io.Copy(upstream, reader)
		upstream.(*net.TCPConn).CloseWrite()
	}()
	io.Copy(client, upstream)
	client.Close()
	<-done
}

var images = map[string]repository{"pull": image("marsa-pull-proof\n"), "build": image("marsa-build-proof\n")}

func main() {
	if len(os.Args) == 2 && os.Args[1] == "--intercept" {
		runInterceptor()
		return
	}
	go func() {
		server := http.Server{Addr: registryAddr, Handler: http.HandlerFunc(registry), ReadHeaderTimeout: 3 * time.Second}
		if err := server.ListenAndServeTLS("/usr/local/lib/marsa-fixture/leaf.pem", "/usr/local/lib/marsa-fixture/leaf.key"); err != nil {
			log.Fatal("fixture registry did not start")
		}
	}()
	server := http.Server{Addr: proxyAddr, Handler: http.HandlerFunc(proxy), ReadHeaderTimeout: 3 * time.Second}
	if err := server.ListenAndServe(); err != nil {
		log.Fatal("fixture proxy did not start")
	}
}

func runInterceptor() {
	const origin = "127.0.0.1:5444"
	cert, err := tls.LoadX509KeyPair("/usr/local/lib/marsa-fixture/leaf.pem", "/usr/local/lib/marsa-fixture/leaf.key")
	if err != nil {
		log.Fatal("could not load disposable fixture identity")
	}
	config := &tls.Config{Certificates: []tls.Certificate{cert}, MinVersion: tls.VersionTLS12}
	go func() {
		backend := &http.Server{Addr: origin, Handler: http.HandlerFunc(registry), ReadHeaderTimeout: 3 * time.Second}
		if err := backend.ListenAndServe(); err != nil {
			log.Fatal("fixture HTTP registry stopped")
		}
	}()
	for _, address := range []string{"127.0.0.1:43129", "127.0.0.1:43130"} {
		go func() {
			server := &http.Server{Addr: address, ReadHeaderTimeout: 3 * time.Second,
				Handler: http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) { relay(w, r, origin, config) })}
			if err := server.ListenAndServe(); err != nil {
				log.Fatal("fixture interceptor stopped")
			}
		}()
	}
	select {}
}
