package main

import (
	"context"
	"encoding/hex"
	"fmt"
	"io"
	"log/slog"
	"math/rand/v2"
	"os"
	"strings"
	"sync"
	"time"

	"github.com/emilbroman/skyr/pkg/confsrv"
	"github.com/emilbroman/skyr/pkg/confsrv/storage"
	kivik "github.com/go-kivik/kivik/v4"
	_ "github.com/go-kivik/kivik/v4/couchdb"
	"github.com/redis/go-redis/v9"

	"github.com/go-git/go-git/v5/plumbing"
	"github.com/go-git/go-git/v5/plumbing/format/pktline"
	"github.com/go-git/go-git/v5/plumbing/object"
	"github.com/go-git/go-git/v5/plumbing/protocol/packp"
	"github.com/go-git/go-git/v5/plumbing/transport"
	"github.com/go-git/go-git/v5/plumbing/transport/server"

	"github.com/fatih/color"
	"github.com/gliderlabs/ssh"

	"github.com/alecthomas/kong"
)

var CLI struct {
	Serve struct{} `cmd:"" help:"Start the Skyr config server"`
	Show  struct {
		Repo string `arg:"" help:"Repository to show"`
	} `cmd:"" help:"Get the status of a given repository"`
	Reconcile struct {
		Repo string `arg:"" help:"Repository to reconcile"`
	} `cmd:"" help:"Run a reconciler daemon"`
	Tree struct {
		Repo   string `arg:"" help:"Repository to view"`
		Branch string `arg:"" help:"Branch ref to view"`
	} `cmd:"" help:"Print the contents of a repository"`
}

func main() {
	ctx := kong.Parse(&CLI)

	client, err := kivik.New("couch", "http://skyr:password123@localhost:5984/")
	if err != nil {
		slog.Error("failed to connect to db", "", err)
		os.Exit(1)
	}

	rdb := redis.NewClient(&redis.Options{
		Addr:     "localhost:6379",
		Password: "",
		DB:       0,
	})

	switch ctx.Command() {
	case "serve":
		slog.SetLogLoggerLevel(slog.LevelDebug)

		srv := server.NewServer(storage.NewCouchDBRepoLoader(context.Background(), client, rdb))

		ssh.Handle(func(s ssh.Session) {
			defer s.Exit(0)

			stderr := s.Stderr()

			pw := pktline.NewEncoder(s)

			cmd := s.Command()[0]
			args := s.Command()[1:]

			switch cmd {
			case "git-receive-pack":
				endpoint, err := transport.NewEndpoint(fmt.Sprintf("git://%s:@localhost%s", s.User(), args[0]))

				if err != nil {
					pw.Encodef("ERR Invalid repo: %s", args[0])
					return
				}

				sess, err := srv.NewReceivePackSession(endpoint, nil)
				if err != nil {
					pw.Encodef("ERR Failed to open session: %v", err)
					return
				}

				slog.Debug("opened session")

				advrefs, err := sess.AdvertisedReferencesContext(s.Context())
				if err != nil {
					pw.Encodef("ERR Failed to list references for advertising: %v", err)
					return
				}

				slog.Debug("listed references", slog.Any("advrefs", advrefs))

				err = advrefs.Encode(s)
				if err != nil {
					pw.Encodef("ERR Failed to advertise references: %v", err)
					return
				}

				slog.Debug("advertised references", slog.Any("advrefs", advrefs))

				req := packp.NewReferenceUpdateRequest()

				slog.Debug("create req", slog.Any("req", req))

				err = req.Decode(confsrv.NewNopReadCloser(s))
				if err != nil {
					pw.Encodef("ERR Failed to decode request: %v", err)
					return
				}

				rollout := make(map[plumbing.ReferenceName]confsrv.Rollout)

				expectPackfile := false
				for _, cmd := range req.Commands {
					if cmd.Action() != packp.Delete {
						expectPackfile = true
						rollout[cmd.Name] = confsrv.RolloutDesired
					} else {
						rollout[cmd.Name] = confsrv.RolloutUndesired
					}
				}

				if !expectPackfile {
					req.Packfile = nil
				}

				status, err := sess.ReceivePack(s.Context(), req)
				if err != nil {
					pw.Encodef("ERR Failed to receive pack: %v", err)
					return
				}

				if status != nil {
					err = status.Encode(s)
					if err != nil {
						pw.Encodef("ERR Failed to report status: %v", err)
						return
					}
				}

				for name, action := range rollout {
					color.New(color.FgCyan).Fprintf(stderr, "%s -> %s\n", action, name)
				}

			// case "git-upload-pack":

			default:
				pw.Encodef("ERR Unsupported command: %s", cmd)
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

	case "show <repo>":
		slog.SetLogLoggerLevel(slog.LevelError)

		repo, err := storage.NewRefsRepo(context.Background(), client, CLI.Show.Repo)
		if err != nil {
			slog.Error("failed to open repo", slog.Any("err", err))
			os.Exit(1)
		}
		iter, err := repo.ListAll(context.Background())
		if err != nil {
			slog.Error("failed to list refs", slog.Any("err", err))
			os.Exit(1)
		}
		err = iter.ForEach(func(r *plumbing.Reference) error {
			ch := make(chan *confsrv.RefDoc)

			fmt.Println(r.Name())

			var wg sync.WaitGroup
			go func() {
				for doc := range ch {
					fmt.Printf("%s: %s @ %s\n", doc.ID, doc.Rollout, doc.Hash)
				}
				wg.Done()
			}()

			wg.Add(1)
			err = repo.RefHistory(context.Background(), r.Name(), ch)
			if err != nil {
				return err
			}

			wg.Wait()

			return nil
		})
		if err != nil {
			slog.Error("failed to show refs", slog.Any("err", err))
			os.Exit(1)
		}

	case "tree <repo> <branch>":
		refs, err := storage.NewRefsRepo(context.Background(), client, CLI.Tree.Repo)
		if err != nil {
			slog.Error("failed to open repo", slog.Any("err", err))
			os.Exit(1)
		}

		doc, err := refs.Get(context.Background(), plumbing.NewBranchReferenceName(CLI.Tree.Branch))
		if err != nil {
			slog.Error("failed to get ref", slog.Any("err", err))
			os.Exit(1)
		}

		if doc == nil {
			slog.Error("ref does not exist", slog.String("branch", CLI.Tree.Branch))
			os.Exit(1)
		}

		objs := storage.NewObjsRepo(rdb, CLI.Tree.Repo)

		storer := storage.NewStorer(context.Background(), refs, objs)

		commit, err := object.GetCommit(storer, doc.ToReference().Hash())
		if err != nil {
			slog.Error("failed to get commit", slog.Any("err", err))
			os.Exit(1)
		}

		tree, err := object.GetTree(storer, commit.TreeHash)

		walker := object.NewTreeWalker(tree, true, nil)

		for name, entry, err := walker.Next(); err != io.EOF; name, entry, err = walker.Next() {
			if err != nil {
				slog.Error("failed while walking tree", slog.Any("err", err))
				os.Exit(1)
			}

			fmt.Println(name, entry)
			blob, err := object.GetBlob(storer, entry.Hash)

			if err != nil {
				slog.Error("failed to get blob", slog.Any("err", err))
				os.Exit(1)
			}

			rd, err := blob.Reader()
			if err != nil {
				slog.Error("failed to get blob reader", slog.Any("err", err))
				os.Exit(1)
			}

			_, err = io.Copy(os.Stdout, rd)
			if err != nil {
				slog.Error("failed to print blob", slog.Any("err", err))
				os.Exit(1)
			}
		}

	case "reconcile <repo>":
		slog.SetLogLoggerLevel(slog.LevelDebug)

		db := client.DB(fmt.Sprintf("repo/refs/%s", CLI.Reconcile.Repo))

		state := make(map[string]<-chan bool)

		for {
			rs := db.AllDocs(context.Background(), kivik.IncludeDocs())
			for rs.Next() {
				var doc confsrv.RefDoc
				err := rs.ScanDoc(&doc)
				if err != nil {
					slog.Error("failed to read ref", slog.Any("err", err))
					os.Exit(1)
				}

				if strings.HasPrefix(doc.ID, "_design/") {
					continue
				}

				seenCh, seen := state[doc.ID]

				if seen {
					select {
					case <-seenCh:
						slog.Debug("completed previous reconciliation, retry", slog.String("id", doc.ID))
						delete(state, doc.ID)
					default:
						slog.Debug("still reconciling", slog.String("id", doc.ID), slog.Int("chlen", len(seenCh)))
						continue
					}
				}

				if doc.Rollout == confsrv.RolloutDown || doc.Rollout == confsrv.RolloutUp {
					// TODO: probe if up
					// Done!
					continue
				}

				ch := make(chan bool, 1)
				state[doc.ID] = ch

				slog.Info("reconcile deployment", slog.String("id", doc.ID))

				switch doc.Rollout {
				case confsrv.RolloutDesired:
					go rollout(ch, &doc, db)

				case confsrv.RolloutLinger:
					go checkLinger(ch, &doc, db)

				case confsrv.RolloutUndesired:
					go teardown(ch, &doc, db)

				default:
					panic(doc.Rollout)
				}
			}

			time.Sleep(time.Second * 2)
		}

	default:
		slog.Error(ctx.Command())
		os.Exit(1)
	}
}

func rollout(done chan<- bool, doc *confsrv.RefDoc, db *kivik.DB) {
	defer func() { done <- true }()
	for rand.Float32() < 0.2 {
		slog.Debug("rolling out", slog.String("doc", doc.ID))
		time.Sleep(time.Second * 3)
	}
	if rand.Float32() < 0.5 {
		var err error
		doc.Rollout = confsrv.RolloutUp
		doc.Rev, err = db.Put(context.Background(), doc.ID, doc)
		if err != nil {
			slog.Error("failed to update record state", slog.String("doc", doc.ID))
		} else {
			slog.Debug("rolled out", slog.String("doc", doc.ID))
		}
	} else {
		slog.Debug("not rolled out yet", slog.String("doc", doc.ID))
	}
}

func teardown(done chan<- bool, doc *confsrv.RefDoc, db *kivik.DB) {
	defer func() { done <- true }()
	for rand.Float32() < 0.2 {
		slog.Debug("tearing down", slog.String("doc", doc.ID))
		time.Sleep(time.Second * 3)
	}
	if rand.Float32() < 0.5 {
		var err error
		doc.Rollout = confsrv.RolloutDown
		doc.Rev, err = db.Put(context.Background(), doc.ID, doc)
		if err != nil {
			slog.Error("failed to update record state", slog.String("doc", doc.ID))
		} else {
			slog.Debug("tore down", slog.String("doc", doc.ID))
		}
	} else {
		slog.Debug("not torn down yet", slog.String("doc", doc.ID))
	}
}

func checkLinger(done chan<- bool, doc *confsrv.RefDoc, db *kivik.DB) {
	defer func() { done <- true }()

	slog.Debug("checking linger", slog.String("doc", doc.ID))

	superDoc := db.Get(context.Background(), doc.SupercededBy)
	if superDoc.Err() != nil {
		slog.Error("failed to get superceding doc", slog.Any("err", superDoc.Err()))
		return
	}

	var super confsrv.RefDoc
	err := superDoc.ScanDoc(&super)
	if err != nil {
		slog.Error("failed to scan superceding doc", slog.Any("err", err))
		return
	}

	switch super.Rollout {
	case confsrv.RolloutDesired:
		slog.Info("superceding deployment not yet up, keep lingering", slog.Any("lingering", doc.ID), slog.Any("superceding", super.ID))
		return

	case confsrv.RolloutLinger:
		slog.Info("superceding deployment is lingering, keep lingering too", slog.Any("lingering", doc.ID), slog.Any("superceding", super.ID))
		return

	case confsrv.RolloutUndesired:
		fallthrough

	case confsrv.RolloutDown:
		slog.Info("superceding deployment not desired, mark as undesired too", slog.Any("lingering", doc.ID), slog.Any("superceding", super.ID))

	case confsrv.RolloutUp:
		slog.Info("superceding deployment is up, mark as undesired", slog.Any("lingering", doc.ID), slog.Any("superceding", super.ID))

	default:
		panic(fmt.Sprintf("unexpected confsrv.Rollout: %#v", super.Rollout))
	}

	doc.Rollout = confsrv.RolloutUndesired
	doc.Rev, err = db.Put(context.Background(), doc.ID, doc)
	if err != nil {
		slog.Error("failed to mark lingering deployment as undesired", slog.Any("err", err))
	}
}
