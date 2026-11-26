package conformance

import (
	"context"
	"io"
	"os"
	"strings"
	"testing"
	"time"

	tc "github.com/testcontainers/testcontainers-go"
	"github.com/testcontainers/testcontainers-go/exec"
)

func TestEngine(t *testing.T) {
	ctx, cancel := context.WithTimeout(t.Context(), 2*time.Minute)
	defer cancel()
	container, err := tc.Run(ctx, os.Getenv("MARSA_SAMPLE_IMAGE"), tc.WithCmd("sleep", "180"))
	tc.CleanupContainer(t, container)
	if err != nil {
		t.Fatal(err)
	}
	code, output, err := container.Exec(ctx, []string{"echo", "marsa-go"}, exec.Multiplexed())
	if err != nil {
		t.Fatal(err)
	}
	result, err := io.ReadAll(output)
	if err != nil || code != 0 || strings.TrimSpace(string(result)) != "marsa-go" {
		t.Fatalf("unexpected exec result: exit=%d output=%q err=%v", code, result, err)
	}
}
