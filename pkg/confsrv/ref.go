package confsrv

import (
	"crypto/rand"
	"fmt"
	"time"

	"github.com/go-git/go-git/v5/plumbing"
)

type Rollout string

const (
	RolloutUp        = Rollout("UP")
	RolloutDesired   = Rollout("DESIRED")
	RolloutLinger    = Rollout("LINGER")
	RolloutUndesired = Rollout("UNDESIRED")
	RolloutDown      = Rollout("DOWN")
)

type RefDoc struct {
	ID  string `json:"_id,omitempty"`
	Rev string `json:"_rev,omitempty"`

	Timestamp time.Time `json:"timestamp"`

	Name           plumbing.ReferenceName `json:"name"`
	Hash           string                 `json:"hash,omitempty"`
	SymbolicTarget plumbing.ReferenceName `json:"symbolic_target,omitempty"`

	Rollout      Rollout `json:"rollout"`
	SupercededBy string  `json:"superceded_by,omitempty"`
}

func CreateRefDoc(newRef *plumbing.Reference) *RefDoc {
	doc := &RefDoc{
		Timestamp: time.Now().In(time.UTC),
		Name:      newRef.Name(),
		Rollout:   RolloutDesired,
	}

	doc.ID = fmt.Sprintf("%s:%s:%s", doc.Name, doc.Timestamp.Format(time.RFC3339), rand.Text())

	fmt.Println(doc)

	if newRef.Type() == plumbing.HashReference {
		doc.Hash = newRef.Hash().String()
	} else {
		doc.SymbolicTarget = newRef.Target()
	}

	return doc
}

func (r *RefDoc) ToReference() *plumbing.Reference {
	if r.Hash != "" {
		return plumbing.NewHashReference(r.Name, plumbing.NewHash(r.Hash))
	}
	return plumbing.NewSymbolicReference(r.Name, r.SymbolicTarget)
}
