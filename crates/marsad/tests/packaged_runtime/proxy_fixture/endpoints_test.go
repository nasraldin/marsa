package main

import (
	"archive/tar"
	"bytes"
	"compress/gzip"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"testing"
)

type descriptor struct {
	Digest string `json:"digest"`
}

func TestRegistryServesDistinctValidImageLayers(t *testing.T) {
	for _, c := range []struct{ name, marker string }{
		{"pull", "marsa-pull-proof\n"},
		{"build", "marsa-build-proof\n"},
	} {
		manifestReq := httptest.NewRequest(http.MethodGet, "https://"+registryName+"/v2/marsa/"+c.name+"/manifests/proof", nil)
		manifestOut := httptest.NewRecorder()
		registry(manifestOut, manifestReq)
		if manifestOut.Code != http.StatusOK {
			t.Fatal("fixture manifest unavailable")
		}
		var manifest struct{ Layers []descriptor }
		if json.Unmarshal(manifestOut.Body.Bytes(), &manifest) != nil || len(manifest.Layers) != 1 {
			t.Fatal("fixture manifest must contain one OCI layer")
		}
		blobReq := httptest.NewRequest(http.MethodGet, "https://"+registryName+"/v2/marsa/"+c.name+"/blobs/"+manifest.Layers[0].Digest, nil)
		blobOut := httptest.NewRecorder()
		registry(blobOut, blobReq)
		gz, err := gzip.NewReader(bytes.NewReader(blobOut.Body.Bytes()))
		if err != nil {
			t.Fatal(err)
		}
		defer gz.Close()
		tarball := tar.NewReader(gz)
		h, err := tarball.Next()
		if err != nil || h.Name != "fixture-marker" {
			t.Fatal("missing fixture marker path")
		}
		data, err := io.ReadAll(tarball)
		if err != nil || string(data) != c.marker {
			t.Fatal("unexpected fixture marker content")
		}
	}
}

func TestProxyRejectsNonFixtureDestination(t *testing.T) {
	req := httptest.NewRequest(http.MethodConnect, "outside-fixture.invalid:443", nil)
	out := httptest.NewRecorder()
	proxy(out, req)
	if out.Code != http.StatusForbidden {
		t.Fatal("proxy allowed an unrelated destination")
	}
}
