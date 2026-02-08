package main

import (
	"context"
	"encoding/binary"
	"encoding/hex"
	"fmt"
	"io"
	"log/slog"
	"os"

	"github.com/emilbroman/skyr/pkg/confsrv"
	"github.com/emilbroman/skyr/pkg/confsrv/storage"
	kivik "github.com/go-kivik/kivik/v4"
	_ "github.com/go-kivik/kivik/v4/couchdb"

	"github.com/go-git/go-git/v5/plumbing/protocol/packp"
	"github.com/go-git/go-git/v5/plumbing/transport"
	"github.com/go-git/go-git/v5/plumbing/transport/server"

	"github.com/gliderlabs/ssh"
)

func pktLine(w io.Writer, content string) error {
	buf := fmt.Appendf(nil, "0000%s\n", content)

	pktLen := uint16(len(buf))
	hex.Encode(buf, binary.BigEndian.AppendUint16(nil, pktLen))

	slog.Debug("writing pkt-line", slog.String("line", string(buf)))

	_, err := w.Write(buf)
	return err
}

func errorLinef(w io.Writer, format string, a ...any) {
	pktLine(w, fmt.Sprintf("ERR %s", fmt.Sprintf(format, a...)))
}

type debugReader struct {
	r io.ReadCloser
}

// Read implements [io.Reader].
func (d debugReader) Read(p []byte) (n int, err error) {
	slog.Debug("read!")
	n, err = d.r.Read(p)
	slog.Debug("read", slog.Any("data", string(p[:n])))
	return
}

func main() {
	slog.SetLogLoggerLevel(slog.LevelDebug)

	client, err := kivik.New("couch", "http://skyr:password123@localhost:5984/")
	if err != nil {
		slog.Error("failed to connect to db", "", err)
		os.Exit(1)
	}

	srv := server.NewServer(storage.NewCouchDBRepoLoader(context.Background(), client))

	ssh.Handle(func(s ssh.Session) {
		defer s.Exit(0)

		cmd := s.Command()[0]
		args := s.Command()[1:]

		switch cmd {
		case "git-receive-pack":
			endpoint, err := transport.NewEndpoint(fmt.Sprintf("git://%s:@localhost%s", s.User(), args[0]))

			if err != nil {
				errorLinef(s, "Invalid repo: %s", args[0])
				return
			}

			sess, err := srv.NewReceivePackSession(endpoint, nil)
			if err != nil {
				errorLinef(s, "Failed to open session: %v", err)
				return
			}

			slog.Debug("opened session")

			advrefs, err := sess.AdvertisedReferencesContext(s.Context())
			if err != nil {
				errorLinef(s, "Failed to list references for advertising: %v", err)
				return
			}

			slog.Debug("listed references", slog.Any("advrefs", advrefs))

			err = advrefs.Encode(s)
			if err != nil {
				errorLinef(s, "Failed to advertise references: %v", err)
				return
			}

			slog.Debug("advertised references", slog.Any("advrefs", advrefs))

			req := packp.NewReferenceUpdateRequest()

			slog.Debug("create req", slog.Any("req", req))

			err = req.Decode(debugReader{r: confsrv.NewNopReadCloser(s)})
			if err != nil {
				errorLinef(s, "Failed to decode request: %v", err)
				return
			}

			onlyDeletes := true
			for _, cmd := range req.Commands {
				if cmd.Action() != packp.Delete {
					onlyDeletes = false
				}
			}

			if onlyDeletes {
				req.Packfile = nil
			}

			status, err := sess.ReceivePack(s.Context(), req)
			if err != nil {
				errorLinef(s, "Failed to receive pack: %v", err)
				return
			}

			if status != nil {
				err = status.Encode(s)
				if err != nil {
					errorLinef(s, "Failed to report status: %v", err)
					return
				}
			}

		// case "git-upload-pack":

		default:
			errorLinef(s, "Unsupported command: %s", cmd)
			return
		}
	})

	err = ssh.ListenAndServe(":2222", nil,
		ssh.HostKeyFile("host.pem"),
		ssh.PublicKeyAuth(func(ctx ssh.Context, key ssh.PublicKey) bool {
			slog.Info("TODO: Auth",
				slog.String("user", ctx.User()),
				slog.String("pubkey", hex.EncodeToString(key.Marshal())),
			)
			return true
		}),
	)
	if err != nil {
		slog.Error("failed to serve", "", err)
		os.Exit(1)
	}
}
