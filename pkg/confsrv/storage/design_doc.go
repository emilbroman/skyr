package storage

import (
	"context"
	"fmt"

	"github.com/go-kivik/kivik/v4"
)

type DesignDoc struct {
	ID    string          `json:"_id"`
	Rev   string          `json:"_rev,omitempty"`
	Views map[string]View `json:"views"`
}

type View struct {
	Map    string `json:"map"`
	Reduce string `json:"reduce,omitempty"`
}

func PutDesignDoc(ctx context.Context, db *kivik.DB, name string, views map[string]View) error {
	docID := fmt.Sprintf("_design/%s", name)

	rev, _ := db.GetRev(ctx, docID)
	_, err := db.Put(ctx, docID, DesignDoc{
		ID:    docID,
		Rev:   rev,
		Views: views,
	})

	return err
}
